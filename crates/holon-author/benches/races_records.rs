//! Records, JSON, dates: each gap of JavaScript's `Object`, `JSON` options, `in`, spread,
//! `typeof` and `Date`, a composite against a native candidate
//! (docs/engineering/themes/js-library-races/records.md).
//!
//! The date composites need `floor` and `trunc`, which no leaf has yet (numbers.md decides them).
//! Each is checked with an exact `trunc` made of texts (`as_text`, `split`, `json_parse`: a
//! number's text cut at its point), and raced with each call stood in by a one-node `x ?? 0`, so it
//! costs what it would with `floor` a leaf. The composites over the date parts (`date_part`, which
//! records.md proposes) and over `utc_time` (the MakeDate arithmetic `date_utc` and `date_set`
//! share) are checked over the exact composites of those, and raced with each call stood in the
//! same way: their values are wrong then, so a stood-in composite is timed, never checked.

mod race;

use std::collections::BTreeMap;

use holon_core::{stringify_json, Value};
use race::{check, function, mapped, num, race, same, text, v, Composite, Side};

// ── values ──────────────────────────────────────────────────────────────────────────────────────

/// A record of `n` keys, `{prefix}0` to `{prefix}{n-1}`, each a number.
fn record(n: usize, prefix: &str) -> Value {
    Value::Record((0..n).map(|i| (format!("{prefix}{i}"), num(i as f64))).collect())
}

fn none() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}

/// Records at their edges: empty, one key, nested, keys whose insertion order isn't their sorted
/// order (`10` before `2`, an astral key before a private-use one in UTF-16 but not in UTF-8), a
/// `-0` and an empty list, and JSON's `null`.
fn edge_records() -> Vec<Value> {
    vec![
        v("{}"),
        v(r#"{"a": 1}"#),
        v(r#"{"b": {"c": [1, {"d": true}]}, "a": "x"}"#),
        v(r#"{"b": 1, "a": 2, "10": 3, "2": 4, "é": 5, "😀": 6, "": 7}"#),
        Value::Record(BTreeMap::from([("z".into(), num(-0.0)), ("y".into(), Value::List(vec![])), ("n".into(), none())])),
    ]
}

fn ones(values: Vec<Value>) -> Vec<Vec<Value>> {
    values.into_iter().map(|x| vec![x]).collect()
}

// ── native candidates: records ──────────────────────────────────────────────────────────────────

/// `Object.values(r)`: the values, in the record's key order.
fn values(xs: &[&Value]) -> Option<Value> {
    let [Value::Record(r)] = xs else { return None };
    Some(Value::List(r.values().cloned().collect()))
}

/// `Object.entries(r)`: `[key, value]` pairs, in the record's key order.
fn entries(xs: &[&Value]) -> Option<Value> {
    let [Value::Record(r)] = xs else { return None };
    Some(Value::List(r.iter().map(|(k, x)| Value::List(vec![text(k), x.clone()])).collect()))
}

/// `Object.fromEntries(es)`: each entry's first item the key (a text), its second the value, a
/// later key replacing an earlier; an entry of fewer than two items, or a key that isn't a text,
/// gives nothing.
fn from_entries(xs: &[&Value]) -> Option<Value> {
    let [Value::List(es)] = xs else { return None };
    let mut r = BTreeMap::new();
    for e in es {
        let Value::List(e) = e else { return None };
        let (Some(Value::Text(k)), Some(x)) = (e.first(), e.get(1)) else { return None };
        r.insert(k.clone(), x.clone());
    }
    Some(Value::Record(r))
}

/// `{ ...a, ...b }`, `Object.assign({}, a, b)`: `a`'s fields and `b`'s, `b`'s where both have one.
fn merge(xs: &[&Value]) -> Option<Value> {
    let [Value::Record(a), Value::Record(b)] = xs else { return None };
    let mut r = a.clone();
    r.extend(b.iter().map(|(k, x)| (k.clone(), x.clone())));
    Some(Value::Record(r))
}

/// `Object.hasOwn(r, k)`, `k in r`: whether the record has the key.
fn has(xs: &[&Value]) -> Option<Value> {
    let [Value::Record(r), Value::Text(k)] = xs else { return None };
    Some(Value::Boolean(r.contains_key(k)))
}

/// `Object.groupBy(xs, (x) => x.g)`: the items in lists by their key, each list in the items'
/// order; an item without a text key gives nothing. (The leaf would apply its function to each
/// item, a node each; this reads `g` directly, which favours it by about that node.)
fn group_by(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    for x in items {
        let Value::Record(fields) = x else { return None };
        let Some(Value::Text(k)) = fields.get("g") else { return None };
        match groups.entry(k.clone()).or_insert_with(|| Value::List(vec![])) {
            Value::List(group) => group.push(x.clone()),
            _ => unreachable!(),
        }
    }
    Some(Value::Record(groups))
}

/// `typeof x`: what JavaScript says of the value Holon's is. JSON's `null` is `none()`, which
/// `typeof` calls "object"; `some(x)` is written as `x` in JSON, so it is `typeof x`. An effect
/// has no JavaScript value.
fn type_of(xs: &[&Value]) -> Option<Value> {
    let [x] = xs else { return None };
    Some(text(match x {
        Value::Number(_) => "number",
        Value::Boolean(_) => "boolean",
        Value::Text(_) => "string",
        Value::List(_) | Value::Record(_) | Value::Sum { arm: 1, .. } => "object",
        Value::Sum { payload, .. } => return type_of(&[payload]),
        Value::Effect { .. } => return None,
    }))
}

/// `Object.is(a, b)`: SameValue, numbers by their bits (`-0` isn't `0`), lists and records by
/// value (JavaScript's identity, which Holon's values haven't).
fn same_value(xs: &[&Value]) -> Option<Value> {
    let [a, b] = xs else { return None };
    Some(Value::Boolean(same(a, b)))
}

// ── native candidates: JSON ─────────────────────────────────────────────────────────────────────

/// `JSON.stringify(v, null, space)`'s gap: a number is that many spaces, at most 10 (truncated
/// first; under 1 none); a text is its first 10 characters.
fn gap(space: &Value) -> Option<String> {
    Some(match space {
        Value::Number(n) => " ".repeat(n.trunc().clamp(0.0, 10.0) as usize),
        Value::Text(s) => s.chars().take(10).collect(),
        _ => return None,
    })
}

/// `JSON.stringify(v, null, space)`: ECMA-262's SerializeJSONObject and SerializeJSONArray layout,
/// each member on a line of its own indented by one more gap, `": "` after a key, an empty list or
/// record written `[]` or `{}`; with no gap, the compact text.
fn stringify_indented(xs: &[&Value]) -> Option<Value> {
    let [x, space] = xs else { return None };
    let gap = gap(space)?;
    let mut out = String::new();
    write_indented(x, &gap, "", &mut out)?;
    Some(Value::Text(out))
}

fn write_indented(x: &Value, gap: &str, indent: &str, out: &mut String) -> Option<()> {
    let inner = format!("{indent}{gap}");
    match x {
        Value::Sum { arm: 0, payload } => return write_indented(payload, gap, indent, out),
        _ if gap.is_empty() => out.push_str(&stringify_json(x)?),
        Value::List(items) if !items.is_empty() => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                out.push_str(&inner);
                write_indented(item, gap, &inner, out)?;
            }
            out.push('\n');
            out.push_str(indent);
            out.push(']');
        }
        Value::Record(fields) if !fields.is_empty() => {
            out.push('{');
            for (i, (k, item)) in fields.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                out.push_str(&inner);
                out.push_str(&stringify_json(&text(k))?);
                out.push_str(": ");
                write_indented(item, gap, &inner, out)?;
            }
            out.push('\n');
            out.push_str(indent);
            out.push('}');
        }
        _ => out.push_str(&stringify_json(x)?),
    }
    Some(())
}

/// `JSON.stringify(v, keys)`: every record, at every depth, written with only the keys of the
/// list that it has, **in the list's order** (a key listed twice counts once); lists are written
/// whole. Items of the list that aren't texts are skipped.
fn stringify_keys(xs: &[&Value]) -> Option<Value> {
    let [x, Value::List(list)] = xs else { return None };
    let mut keys: Vec<&str> = Vec::new();
    for k in list {
        if let Value::Text(k) = k
            && !keys.contains(&k.as_str())
        {
            keys.push(k);
        }
    }
    let mut out = String::new();
    write_keys(x, &keys, &mut out)?;
    Some(Value::Text(out))
}

fn write_keys(x: &Value, keys: &[&str], out: &mut String) -> Option<()> {
    match x {
        Value::Sum { arm: 0, payload } => write_keys(payload, keys, out)?,
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_keys(item, keys, out)?;
            }
            out.push(']');
        }
        Value::Record(fields) => {
            out.push('{');
            let mut first = true;
            for k in keys {
                if let Some(item) = fields.get(*k) {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    out.push_str(&stringify_json(&text(k))?);
                    out.push(':');
                    write_keys(item, keys, out)?;
                }
            }
            out.push('}');
        }
        _ => out.push_str(&stringify_json(x)?),
    }
    Some(())
}

// ── native candidates: dates ────────────────────────────────────────────────────────────────────
//
// A date is its time value, as in JavaScript: a whole number of milliseconds since 1970-01-01 UTC,
// within ±8.64e15. Every part is computed in i64, exact over that whole range, on the proleptic
// Gregorian calendar (H. Hinnant's days_from_civil and civil_from_days).

const MS_PER_DAY: i64 = 86_400_000;
const MAX_TIME: f64 = 8.64e15;

/// The time value `x` is, or none: a whole number within ±8.64e15 (`-0` is `0`).
fn time_value(x: &Value) -> Option<i64> {
    let Value::Number(t) = x else { return None };
    (t.fract() == 0.0 && t.abs() <= MAX_TIME).then_some(*t as i64)
}

/// ToIntegerOrInfinity on a finite number: truncated, `-0` made `0`.
fn to_integer(x: f64) -> f64 {
    x.trunc() + 0.0
}

/// TimeClip: none beyond ±8.64e15 (or not finite), else the whole number, `-0` made `0`.
fn clip(tv: f64) -> Option<f64> {
    (tv.is_finite() && tv.abs() <= MAX_TIME).then(|| to_integer(tv))
}

/// Days since 1970-01-01 of a civil date, month 1 to 12.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * if m > 2 { m - 3 } else { m + 9 } + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01: year, month 1 to 12, day 1 to 31.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// The UTC parts of a time value: year, month (0 to 11, as JavaScript counts), date, weekday (0 is
/// Sunday), hours, minutes, seconds, milliseconds.
fn parts(t: i64) -> [i64; 8] {
    let day = t.div_euclid(MS_PER_DAY);
    let ms = t.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(day);
    [y, m - 1, d, (day + 4).rem_euclid(7), ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000]
}

/// `date_part {part}`: one UTC part of a time value, as each `getUTC…` gives it.
fn part(xs: &[&Value], k: usize) -> Option<Value> {
    let [t] = xs else { return None };
    Some(num(parts(time_value(t)?)[k] as f64))
}
fn utc_year(xs: &[&Value]) -> Option<Value> {
    part(xs, 0)
}
fn utc_month(xs: &[&Value]) -> Option<Value> {
    part(xs, 1)
}
fn utc_date(xs: &[&Value]) -> Option<Value> {
    part(xs, 2)
}
fn utc_weekday(xs: &[&Value]) -> Option<Value> {
    part(xs, 3)
}
fn utc_hours(xs: &[&Value]) -> Option<Value> {
    part(xs, 4)
}
fn utc_minutes(xs: &[&Value]) -> Option<Value> {
    part(xs, 5)
}
fn utc_seconds(xs: &[&Value]) -> Option<Value> {
    part(xs, 6)
}
fn utc_ms(xs: &[&Value]) -> Option<Value> {
    part(xs, 7)
}

/// MakeDay, ECMA-262 §21.4.1.28, with V8's bounds: the year (truncated) within ±1,000,000 and the
/// month within ±10,000,000, the month carried into the year, then the day count of the month's
/// first day plus `date - 1` in IEEE arithmetic.
fn make_day(year: f64, month: f64, date: f64) -> Option<f64> {
    let (y, m) = (to_integer(year), to_integer(month));
    if !(-1e6..=1e6).contains(&y) || !(-1e7..=1e7).contains(&m) {
        return None;
    }
    let (y, m) = (y as i64 + (m as i64).div_euclid(12), (m as i64).rem_euclid(12));
    Some(days_from_civil(y, m + 1, 1) as f64 + to_integer(date) - 1.0)
}

/// MakeTime, §21.4.1.27: `((h * 3600000 + m * 60000) + s * 1000) + ms`, each truncated first, in
/// IEEE arithmetic and that order.
fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    to_integer(h) * 3_600_000.0 + to_integer(m) * 60_000.0 + to_integer(s) * 1000.0 + to_integer(ms)
}

fn numbers<const N: usize>(xs: &[&Value]) -> Option<[f64; N]> {
    let mut out = [0.0; N];
    if xs.len() != N {
        return None;
    }
    for (o, x) in out.iter_mut().zip(xs) {
        let Value::Number(n) = x else { return None };
        *o = *n;
    }
    Some(out)
}

/// `utc_time`: TimeClip(MakeDate(MakeDay(year, month, date), MakeTime(h, m, s, ms))), where
/// MakeDate is `day * 86400000 + time`. What `date_utc` (`Date.UTC`) computes after its year
/// mapping, and what `date_set` (every `setUTC…`) computes with the parts it isn't given read from
/// the date.
fn utc_time(xs: &[&Value]) -> Option<Value> {
    let [y, mo, d, h, mi, s, ms] = numbers::<7>(xs)?;
    clip(make_day(y, mo, d)? * 86_400_000.0 + make_time(h, mi, s, ms)).map(num)
}

/// `Date.UTC(y, m, d, h, mi, s, ms)`: a year from 0 to 99 (truncated) is 1900 plus it.
fn date_utc(xs: &[&Value]) -> Option<Value> {
    let [y, mo, d, h, mi, s, ms] = numbers::<7>(xs)?;
    let yi = to_integer(y);
    let y = if (0.0..=99.0).contains(&yi) { 1900.0 + yi } else { y };
    utc_time(&[&num(y), &num(mo), &num(d), &num(h), &num(mi), &num(s), &num(ms)])
}

/// `date_part {part: "time"}`, `new Date(ms)`'s time value: TimeClip.
fn time_clip(xs: &[&Value]) -> Option<Value> {
    let [Value::Number(ms)] = xs else { return None };
    clip(*ms).map(num)
}

/// `date_set {part}`, `setUTC…` as a value: the date with part `k` (0 year to 6 milliseconds, the
/// order `utc_time` takes them in) replaced by `x`, through MakeDate and TimeClip as JavaScript
/// computes it.
fn set_part(xs: &[&Value], k: usize) -> Option<Value> {
    let [t, Value::Number(x)] = xs else { return None };
    let p = parts(time_value(t)?);
    let mut args = [p[0], p[1], p[2], p[4], p[5], p[6], p[7]].map(|n| num(n as f64));
    args[k] = num(*x);
    utc_time(&args.iter().collect::<Vec<_>>())
}
fn set_utc_full_year(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 0)
}
fn set_utc_month(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 1)
}
fn set_utc_date(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 2)
}
fn set_utc_hours(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 3)
}
fn set_utc_minutes(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 4)
}
fn set_utc_seconds(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 5)
}
fn set_utc_milliseconds(xs: &[&Value]) -> Option<Value> {
    set_part(xs, 6)
}
/// `d.setUTCHours(7)`, as a unary candidate for a `map`.
fn set_utc_hours_7(xs: &[&Value]) -> Option<Value> {
    let [t] = xs else { return None };
    set_utc_hours(&[t, &num(7.0)])
}

/// `date_text {format: "iso"}`, `toISOString`: `YYYY-MM-DDTHH:mm:ss.sssZ`, a year outside 0 to
/// 9999 as a sign and six digits.
fn iso(xs: &[&Value]) -> Option<Value> {
    let [t] = xs else { return None };
    let [y, mo, d, _, h, mi, s, ms] = parts(time_value(t)?);
    let year = match y {
        0..=9999 => format!("{y:04}"),
        _ if y < 0 => format!("-{:06}", -y),
        _ => format!("+{y:06}"),
    };
    Some(text(&format!("{year}-{:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z", mo + 1)))
}

/// `date_text {format: "utc"}`, `toUTCString`: `Thu, 01 Jan 1970 00:00:00 GMT`, a negative year
/// as `-` and four digits.
fn utc_string(xs: &[&Value]) -> Option<Value> {
    let [t] = xs else { return None };
    let [y, mo, d, wd, h, mi, s, _] = parts(time_value(t)?);
    let day = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][wd as usize];
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][mo as usize];
    let year = if y >= 0 { format!("{y:04}") } else { format!("-{:04}", -y) };
    Some(text(&format!("{day}, {d:02} {month} {year} {h:02}:{mi:02}:{s:02} GMT")))
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `date_parse`, `Date.parse(s)`: ECMA-262's Date Time String Format (§21.4.1.32) exactly. `YYYY`, `YYYY-MM` or
/// `YYYY-MM-DD` (a year may be `±YYYYYY`, never `-000000`), then optionally `THH:mm`, `:ss` and
/// `.sss`, and `Z` or `±HH:mm`; `24:00` (all zeros after it) is the end of the day. A date alone is
/// UTC; a time without an offset is local time in JavaScript, and Holon has no local time, so it
/// gives nothing, as does anything outside the format (where an engine falls back to its own
/// heuristics) or beyond ±8.64e15.
fn date_parse(xs: &[&Value]) -> Option<Value> {
    let [Value::Text(s)] = xs else { return None };
    let b = s.as_bytes();
    let mut i = 0usize;
    let digits = |i: &mut usize, n: usize| -> Option<i64> {
        let run = b.get(*i..*i + n)?;
        if !run.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *i += n;
        Some(run.iter().fold(0, |a, d| a * 10 + i64::from(d - b'0')))
    };
    let year = match b.first()? {
        &c @ (b'+' | b'-') => {
            i = 1;
            let y = digits(&mut i, 6)?;
            if c == b'-' && y == 0 {
                return None;
            }
            if c == b'-' { -y } else { y }
        }
        _ => digits(&mut i, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = digits(&mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            day = digits(&mut i, 2)?;
        }
    }
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let date = days_from_civil(year, month, day) * MS_PER_DAY;
    if i == b.len() {
        return clip(date as f64).map(num);
    }
    if b.get(i) != Some(&b'T') {
        return None;
    }
    i += 1;
    let h = digits(&mut i, 2)?;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    let mi = digits(&mut i, 2)?;
    let (mut sec, mut ms) = (0, 0);
    if b.get(i) == Some(&b':') {
        i += 1;
        sec = digits(&mut i, 2)?;
        if b.get(i) == Some(&b'.') {
            i += 1;
            ms = digits(&mut i, 3)?;
        }
    }
    if h > 24 || mi > 59 || sec > 59 || (h == 24 && (mi, sec, ms) != (0, 0, 0)) {
        return None;
    }
    let offset = match b.get(i) {
        Some(b'Z') => {
            i += 1;
            0
        }
        Some(&c @ (b'+' | b'-')) => {
            i += 1;
            let oh = digits(&mut i, 2)?;
            if b.get(i) != Some(&b':') {
                return None;
            }
            i += 1;
            let om = digits(&mut i, 2)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let o = (oh * 60 + om) * 60_000;
            if c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    if i != b.len() {
        return None;
    }
    clip((date + ((h * 60 + mi) * 60 + sec) * 1000 + ms - offset) as f64).map(num)
}

// ── composites ──────────────────────────────────────────────────────────────────────────────────

const VALUES: &str = "function values_of(r) {
  return Object.keys(r).map((k) => r[k])
}";

const ENTRIES: &str = "function entries_of(r) {
  return Object.keys(r).map((k) => [k, r[k]])
}";

const FROM_ENTRIES: &str = "import { insert, get } from \"holon\"
function from_entries(es) {
  return es.reduce((acc, e) => insert(acc, get(e, 0), get(e, 1)), {})
}";

const MERGE: &str = "import { insert } from \"holon\"
function merge(a, b) {
  return Object.keys(b).reduce((acc, k) => insert(acc, k, b[k]), a)
}";

const HAS: &str = "import { is_truthy } from \"holon\"
function has(r, k) {
  return is_truthy([r[k]]) ?? false
}";

const GROUP_BY: &str = "import { insert } from \"holon\"
function add_to_group(acc, x) {
  const k = x.g
  return insert(acc, k, [...(acc[k] ?? []), x])
}
function group_by(xs) {
  return xs.reduce(add_to_group, {})
}";

const TYPE_OF: &str = r#"import { to_text, slice } from "holon"
function type_of(x) {
  const t = to_text(x)
  const c = slice(t, 0, 1)
  return c === "\"" ? "string" : c === "[" || c === "{" || c === "s" ? "object" : t === "true" || t === "false" ? "boolean" : "number"
}"#;

/// An exact `trunc` and `floor` of today's leaves: a number's text (`as_text` writes every finite
/// number without an exponent) cut at its point, read back by `json_parse`; `-0` and the
/// fractions between `-1` and `0` keep their sign through `x * 0`.
const FLOOR: &str = "import { as_text, split, head, json_parse } from \"holon\"
function trunc(x) {
  const t = json_parse(head(split(as_text(x), \".\")))
  return t === 0 ? x * 0 : t
}
function floor(x) {
  const t = trunc(x)
  return x < t ? t - 1 : t
}";

/// `new Date(ms)`: TimeClip.
const TIME_CLIP: &str = "function time_clip(ms) {
  return ms >= -8640000000000000 && ms <= 8640000000000000 ? trunc(ms) + 0 : undefined
}";

/// The `getUTC…` parts of a time value, over `floor`. Each divides by a whole number of
/// milliseconds below 2^53, where `floor(t / d)` is exact (ULP(t / d) / 2 < 1 / d for every
/// |t| <= 8.64e15), and each modulo is `x - floor(x / n) * n`.
const UTC_MS: &str = "function utc_ms(t) {
  return t - floor(t / 1000) * 1000
}";
const UTC_SECONDS: &str = "function utc_seconds(t) {
  const s = floor(t / 1000)
  return s - floor(s / 60) * 60
}";
const UTC_MINUTES: &str = "function utc_minutes(t) {
  const m = floor(t / 60000)
  return m - floor(m / 60) * 60
}";
const UTC_HOURS: &str = "function utc_hours(t) {
  const h = floor(t / 3600000)
  return h - floor(h / 24) * 24
}";
const UTC_WEEKDAY: &str = "function utc_weekday(t) {
  const d = floor(t / 86400000) + 4
  return d - floor(d / 7) * 7
}";

/// civil_from_days, as the year, the month and the date each need it.
fn civil(name: &str, answer: &str) -> String {
    format!(
        "function {name}(t) {{
  const z = floor(t / 86400000) + 719468
  const era = floor(z / 146097)
  const doe = z - era * 146097
  const yoe = floor((doe - floor(doe / 1460) + floor(doe / 36524) - floor(doe / 146096)) / 365)
  const doy = doe - (365 * yoe + floor(yoe / 4) - floor(yoe / 100))
  const mp = floor((5 * doy + 2) / 153)
  return {answer}
}}"
    )
}

/// MakeDay, MakeTime, MakeDate and TimeClip over `floor` and `trunc`, in JavaScript's order of
/// IEEE operations (an `add` chain starts from 0, which changes only the sign of a zero, and
/// TimeClip's `+ 0` drops that).
const UTC_TIME: &str = "function utc_time(year, month, date, h, mi, s, ms) {
  const y = trunc(year)
  const mo = trunc(month)
  const ym = y + floor(mo / 12)
  const mn = mo - floor(mo / 12) * 12
  const yy = mn < 2 ? ym - 1 : ym
  const era = floor(yy / 400)
  const yoe = yy - era * 400
  const doy = floor((153 * (mn < 2 ? mn + 10 : mn - 2) + 2) / 5)
  const doe = yoe * 365 + floor(yoe / 4) - floor(yoe / 100) + doy
  const day = era * 146097 + doe - 719468 + trunc(date) - 1
  const time = trunc(h) * 3600000 + trunc(mi) * 60000 + trunc(s) * 1000 + trunc(ms)
  const tv = day * 86400000 + time
  return y >= -1000000 && y <= 1000000 && mo >= -10000000 && mo <= 10000000 && tv >= -8640000000000000 && tv <= 8640000000000000 ? tv + 0 : undefined
}";

/// `Date.UTC`, over `utc_time`.
const DATE_UTC: &str = "function date_utc(y, mo, d, h, mi, s, ms) {
  return utc_time(y > -1 && y < 100 ? 1900 + trunc(y) : y, mo, d, h, mi, s, ms)
}";

/// The `setUTC…` value forms, over `utc_time` and the parts.
const SETTERS: &str = "function set_utc_full_year(t, x) {
  return utc_time(x, utc_month(t), utc_date(t), utc_hours(t), utc_minutes(t), utc_seconds(t), utc_ms(t))
}
function set_utc_month(t, x) {
  return utc_time(utc_year(t), x, utc_date(t), utc_hours(t), utc_minutes(t), utc_seconds(t), utc_ms(t))
}
function set_utc_date(t, x) {
  return utc_time(utc_year(t), utc_month(t), x, utc_hours(t), utc_minutes(t), utc_seconds(t), utc_ms(t))
}
function set_utc_hours(t, x) {
  return utc_time(utc_year(t), utc_month(t), utc_date(t), x, utc_minutes(t), utc_seconds(t), utc_ms(t))
}
function set_utc_minutes(t, x) {
  return utc_time(utc_year(t), utc_month(t), utc_date(t), utc_hours(t), x, utc_seconds(t), utc_ms(t))
}
function set_utc_seconds(t, x) {
  return utc_time(utc_year(t), utc_month(t), utc_date(t), utc_hours(t), utc_minutes(t), x, utc_ms(t))
}
function set_utc_milliseconds(t, x) {
  return utc_time(utc_year(t), utc_month(t), utc_date(t), utc_hours(t), utc_minutes(t), utc_seconds(t), x)
}";

/// `toISOString`, over the parts.
const ISO: &str = "function pad2(n) {
  return n < 10 ? `0${n}` : `${n}`
}
function pad3(n) {
  return n < 10 ? `00${n}` : n < 100 ? `0${n}` : `${n}`
}
function pad6(n) {
  return n < 10 ? `00000${n}` : n < 100 ? `0000${n}` : n < 1000 ? `000${n}` : n < 10000 ? `00${n}` : n < 100000 ? `0${n}` : `${n}`
}
function year_text(y) {
  return y < 0 ? `-${pad6(0 - y)}` : y > 9999 ? `+${pad6(y)}` : y < 10 ? `000${y}` : y < 100 ? `00${y}` : y < 1000 ? `0${y}` : `${y}`
}
function iso(t) {
  return `${year_text(utc_year(t))}-${pad2(utc_month(t) + 1)}-${pad2(utc_date(t))}T${pad2(utc_hours(t))}:${pad2(utc_minutes(t))}:${pad2(utc_seconds(t))}.${pad3(utc_ms(t))}Z`
}";

/// Every date part, exact over `FLOOR`.
fn parts_ts() -> String {
    [
        UTC_MS.to_string(),
        UTC_SECONDS.into(),
        UTC_MINUTES.into(),
        UTC_HOURS.into(),
        UTC_WEEKDAY.into(),
        civil("utc_year", "mp < 10 ? yoe + era * 400 : yoe + era * 400 + 1"),
        civil("utc_month", "mp < 10 ? mp + 2 : mp - 10"),
        civil("utc_date", "doy - floor((153 * mp + 2) / 5) + 1"),
    ]
    .join("\n")
}

/// `src` with each call of `name` rewritten by `with`, from its arguments as written: how a
/// composite over a leaf that doesn't exist yet is timed, the leaf stood in by a one-node leaf.
fn stand_in(src: &str, name: &str, with: &dyn Fn(&[&str]) -> String) -> String {
    let pat = format!("{name}(");
    let mut out = String::new();
    let mut rest = src;
    while let Some(at) = rest.find(&pat) {
        let boundary = rest[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        out.push_str(&rest[..at]);
        let after = &rest[at + pat.len()..];
        if !boundary {
            out.push_str(&pat);
            rest = after;
            continue;
        }
        let (args, used) = arguments(after);
        let args: Vec<String> = args.iter().map(|a| stand_in(a, name, with)).collect();
        out.push_str(&with(&args.iter().map(String::as_str).collect::<Vec<_>>()));
        rest = &after[used..];
    }
    out.push_str(rest);
    out
}

/// The arguments of a call whose `(` is just before `s`, split at its own commas, and how many
/// bytes they and the closing `)` take.
fn arguments(s: &str) -> (Vec<&str>, usize) {
    let (mut depth, mut start, mut args, mut quote, mut escaped) = (0i32, 0, Vec::new(), None, false);
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(_), _) if escaped => escaped = false,
            (Some(_), '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '`') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')') if depth == 0 => {
                args.push(s[start..i].trim());
                return (args.into_iter().filter(|a| !a.is_empty()).collect(), i + 1);
            }
            (None, ')' | ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => {
                args.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    panic!("no closing parenthesis in {s}")
}

/// A one-node stand-in for a unary leaf: `x ?? 0`, a `default`, passing `x` through.
fn one_node(args: &[&str]) -> String {
    format!("(({}) ?? 0)", args[0])
}

/// A one-node stand-in for a leaf of many inputs: one variadic `add`.
fn one_add(args: &[&str]) -> String {
    format!("({})", args.iter().map(|a| format!("({a})")).collect::<Vec<_>>().join(" + "))
}

/// `src` with each of `names` stood in by `with`.
fn standing(src: &str, names: &[&str], with: &dyn Fn(&[&str]) -> String) -> String {
    names.iter().fold(src.to_string(), |s, n| stand_in(&s, n, with))
}

const PART_NAMES: &[&str] = &["utc_year", "utc_month", "utc_date", "utc_weekday", "utc_hours", "utc_minutes", "utc_seconds", "utc_ms"];

// ── races ───────────────────────────────────────────────────────────────────────────────────────

const SIZES: &[usize] = &[8, 64, 512];

fn race_records(name: &str, c: &Composite, native: race::Native) {
    race(name, &Side::Composite(c), &Side::Native(native), SIZES, |n| vec![record(n, "k")]);
}

/// `n` time values, a day and a bit apart from 2025-11-06.
fn times(n: usize) -> Value {
    Value::List((0..n).map(|k| num(1_762_430_400_123.0 + k as f64 * 86_461_001.0)).collect())
}

/// Check `c` against `native` on `cases`, then race it on the scalar `at`.
fn scalar(name: &str, exact: &Composite, timed: &Composite, native: race::Native, cases: &[Vec<Value>], at: Vec<Value>) {
    check(name, &Side::Composite(exact), &Side::Native(native), cases);
    race(name, &Side::Composite(timed), &Side::Native(native), &[1], move |_| at.clone());
}

/// Race the unary composite `name` of `src` where it is hot: applied to each of 64 time values
/// by `map`, against the candidate applied by `map`.
fn hot(js: &str, src: &str, name: &str, native: race::Native) {
    let c = mapped(src, name);
    race(&format!("{js}, mapped"), &Side::Composite(&c), &Side::NativeMapped(native), &[64], |n| vec![times(n)]);
}

fn main() {
    let edges = ones(edge_records());

    // Object.values, Object.entries: a `map` over `keys`, a `lookup` each.
    let values_c = function(VALUES, "values_of", 1);
    check("Object.values", &Side::Composite(&values_c), &Side::Native(values), &edges);
    race_records("Object.values", &values_c, values);
    let entries_c = function(ENTRIES, "entries_of", 1);
    check("Object.entries", &Side::Composite(&entries_c), &Side::Native(entries), &edges);
    race_records("Object.entries", &entries_c, entries);

    // Object.fromEntries: a `fold` of `insert`, which copies the record each step.
    let from_c = function(FROM_ENTRIES, "from_entries", 1);
    let from_cases: Vec<Vec<Value>> = ones(vec![
        v("[]"),
        v(r#"[["a", 1]]"#),
        v(r#"[["b", 1], ["a", {"c": [2]}], ["10", 3], ["2", 4]]"#),
        v(r#"[["a", 1], ["a", 2]]"#),
        v(r#"[["a", 1, 9]]"#),
        v(r#"[["a"]]"#),
        v(r#"[[1, 2]]"#),
        Value::List(vec![Value::List(vec![text("z"), num(-0.0)])]),
    ]);
    check("Object.fromEntries", &Side::Composite(&from_c), &Side::Native(from_entries), &from_cases);
    race("Object.fromEntries", &Side::Composite(&from_c), &Side::Native(from_entries), SIZES, |n| {
        vec![Value::List((0..n).map(|i| Value::List(vec![text(&format!("k{i}")), num(i as f64)])).collect())]
    });

    // { ...a, ...b }, Object.assign: a `fold` of `insert` over `b`'s keys, from `a`.
    let merge_c = function(MERGE, "merge", 2);
    let mut merge_cases: Vec<Vec<Value>> = vec![
        vec![v("{}"), v("{}")],
        vec![v(r#"{"a": 1}"#), v("{}")],
        vec![v("{}"), v(r#"{"a": 1}"#)],
        vec![v(r#"{"a": 1, "b": 2}"#), v(r#"{"b": 3, "c": 4}"#)],
        vec![v(r#"{"a": {"x": 1}}"#), v(r#"{"a": {"y": 2}}"#)],
    ];
    for r in edge_records() {
        merge_cases.push(vec![r.clone(), v(r#"{"a": 0, "2": 0}"#)]);
        merge_cases.push(vec![v(r#"{"a": 0, "2": 0}"#), r]);
    }
    check("{ ...a, ...b }", &Side::Composite(&merge_c), &Side::Native(merge), &merge_cases);
    race("{ ...a, ...b }", &Side::Composite(&merge_c), &Side::Native(merge), SIZES, |n| {
        let b = Value::Record((0..n).map(|i| (if i % 2 == 0 { format!("a{i}") } else { format!("b{i}") }, num(-(i as f64)))).collect());
        vec![record(n, "a"), b]
    });

    // k in r, Object.hasOwn(r, k): whether `lookup` gives a value.
    let has_c = function(HAS, "has", 2);
    let mut has_cases: Vec<Vec<Value>> = Vec::new();
    for r in edge_records() {
        for k in ["a", "b", "c", "10", "2", "😀", "", "n", "z"] {
            has_cases.push(vec![r.clone(), text(k)]);
        }
    }
    has_cases.push(vec![v(r#"{"": 0}"#), text("")]);
    check("Object.hasOwn", &Side::Composite(&has_c), &Side::Native(has), &has_cases);
    race("Object.hasOwn", &Side::Composite(&has_c), &Side::Native(has), SIZES, |n| vec![record(n, "k"), text(&format!("k{}", n / 2))]);
    // Where it is hot, in a `map` over keys (half of them missing): the composite as a node of the
    // map's function, against a `lookup` there, which stands in for a `has` leaf (the same search,
    // then a boolean instead of a clone of a number).
    let mut each_has = function(&format!("{HAS}\nfunction each_has(r, ks) {{\n  return ks.map((k) => has(r, k))\n}}"), "each_has", 2);
    each_has.name = "ks.map((k) => has(r, k))".into();
    let mut each_lookup = function("function each_lookup(r, ks) {\n  return ks.map((k) => r[k])\n}", "each_lookup", 2);
    each_lookup.name = "ks.map((k) => r[k]), for a has leaf".into();
    race("Object.hasOwn, mapped", &Side::Composite(&each_has), &Side::Composite(&each_lookup), SIZES, |n| {
        vec![record(n, "k"), Value::List((0..n).map(|i| text(&format!("k{}", 2 * i))).collect())]
    });

    // Object.groupBy: a `fold` of `insert` and `append`, which copy the record and the group.
    let group_c = function(GROUP_BY, "group_by", 1);
    let group_cases: Vec<Vec<Value>> = ones(vec![
        v("[]"),
        v(r#"[{"g": "x", "v": 1}]"#),
        v(r#"[{"g": "x", "v": 1}, {"g": "y", "v": 2}, {"g": "x", "v": 3}, {"g": "10", "v": 4}, {"g": "2", "v": 5}]"#),
        v(r#"[{"g": "x"}, {"v": 2}]"#),
        v(r#"[{"g": 1}]"#),
    ]);
    check("Object.groupBy", &Side::Composite(&group_c), &Side::Native(group_by), &group_cases);
    race("Object.groupBy", &Side::Composite(&group_c), &Side::Native(group_by), SIZES, |n| {
        let groups = (n / 4).max(1);
        vec![Value::List((0..n).map(|i| v(&format!(r#"{{"g": "g{}", "v": {i}}}"#, i % groups))).collect())]
    });

    // typeof: the first character of `to_text`, which writes the whole value.
    let type_c = function(TYPE_OF, "type_of", 1);
    let type_cases = ones(vec![
        num(0.0),
        num(-0.0),
        num(-1.5),
        num(1e300),
        text(""),
        text("x"),
        text("\"q"),
        text("true"),
        text("[1]"),
        Value::Boolean(true),
        Value::Boolean(false),
        v("[]"),
        v("[1, [2]]"),
        v("{}"),
        v(r#"{"a": {"b": 1}}"#),
        none(),
    ]);
    check("typeof", &Side::Composite(&type_c), &Side::Native(type_of), &type_cases);
    race("typeof, a number", &Side::Composite(&type_c), &Side::Native(type_of), &[1], |_| vec![num(1234.5)]);
    hot("typeof, a number", TYPE_OF, "type_of", type_of);
    race_records("typeof, a record", &type_c, type_of);

    // Object.is: no composite (no leaf tells -0 from 0); the candidate's meaning, pinned.
    for (a, b, is) in [(num(0.0), num(-0.0), false), (num(-0.0), num(-0.0), true), (num(1.0), num(1.0), true), (v(r#"{"a": [0]}"#), v(r#"{"a": [0]}"#), true), (text("1"), num(1.0), false)] {
        assert_eq!(same_value(&[&a, &b]), Some(Value::Boolean(is)), "Object.is({a:?}, {b:?})");
    }

    // JSON.stringify(v, null, space) and JSON.stringify(v, keys): no composite (each reaches every
    // depth, and no function calls itself); the candidates' layouts, pinned against what
    // JavaScript writes for the same keys in the same order.
    let doc = v(r#"{"a": "x", "b": [1, {}, []], "c": {"d": true}}"#);
    let indented = |space: Value| match stringify_indented(&[&doc, &space]) {
        Some(Value::Text(s)) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(indented(num(2.0)), "{\n  \"a\": \"x\",\n  \"b\": [\n    1,\n    {},\n    []\n  ],\n  \"c\": {\n    \"d\": true\n  }\n}");
    assert_eq!(indented(num(2.9)), indented(num(2.0)));
    assert_eq!(indented(text("\t")), "{\n\t\"a\": \"x\",\n\t\"b\": [\n\t\t1,\n\t\t{},\n\t\t[]\n\t],\n\t\"c\": {\n\t\t\"d\": true\n\t}\n}");
    assert_eq!(indented(num(0.0)), r#"{"a":"x","b":[1,{},[]],"c":{"d":true}}"#);
    assert_eq!(indented(num(-3.0)), indented(num(0.0)));
    assert_eq!(indented(num(20.0)), indented(text("          ")));
    assert_eq!(indented(text("abcdefghijkl")).lines().nth(1), Some("abcdefghij\"a\": \"x\","));
    let keys = |list: &str| match stringify_keys(&[&v(r#"{"b": 1, "a": {"b": 2, "c": 3}, "d": [{"a": 4, "e": 5}]}"#), &v(list)]) {
        Some(Value::Text(s)) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(keys(r#"["b", "a"]"#), r#"{"b":1,"a":{"b":2}}"#);
    assert_eq!(keys(r#"["d", "a", "a", 7]"#), r#"{"d":[{"a":4}],"a":{}}"#);
    assert_eq!(keys("[]"), "{}");

    // Dates. Known answers first: what JavaScript gives for each, so the candidates mean it.
    let t = |x: f64| num(x);
    let answer = |f: race::Native, args: &[Value]| f(&args.iter().collect::<Vec<_>>());
    for (ms, iso_text) in [
        (0.0, "1970-01-01T00:00:00.000Z"),
        (-1.0, "1969-12-31T23:59:59.999Z"),
        (951_782_400_000.0, "2000-02-29T00:00:00.000Z"),
        (-62_135_596_800_000.0, "0001-01-01T00:00:00.000Z"),
        (-62_167_219_200_000.0, "0000-01-01T00:00:00.000Z"),
        (-62_198_755_200_000.0, "-000001-01-01T00:00:00.000Z"),
        (253_402_300_799_999.0, "9999-12-31T23:59:59.999Z"),
        (253_402_300_800_000.0, "+010000-01-01T00:00:00.000Z"),
        (8.64e15, "+275760-09-13T00:00:00.000Z"),
        (-8.64e15, "-271821-04-20T00:00:00.000Z"),
    ] {
        assert_eq!(answer(iso, &[t(ms)]), Some(text(iso_text)), "toISOString of {ms}");
        assert_eq!(answer(date_parse, &[text(iso_text)]), Some(t(ms)), "Date.parse({iso_text})");
    }
    assert_eq!(answer(iso, &[t(8.64e15 + 1.0)]), None);
    assert_eq!(answer(utc_string, &[t(0.0)]), Some(text("Thu, 01 Jan 1970 00:00:00 GMT")));
    assert_eq!(answer(utc_string, &[t(951_782_400_000.0)]), Some(text("Tue, 29 Feb 2000 00:00:00 GMT")));
    assert_eq!(answer(utc_weekday, &[t(0.0)]), Some(t(4.0)));
    for (args, ms) in [
        ([2000.0, 1.0, 29.0, 0.0, 0.0, 0.0, 0.0], Some(951_782_400_000.0)),
        ([99.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0], Some(915_148_800_000.0)),
        ([2020.0, 12.0, 1.0, 0.0, 0.0, 0.0, 0.0], Some(1_609_459_200_000.0)),
        ([2021.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], Some(1_609_372_800_000.0)),
        ([275_760.0, 8.0, 13.0, 0.0, 0.0, 0.0, 0.0], Some(8.64e15)),
        ([275_760.0, 8.0, 13.0, 0.0, 0.0, 0.0, 1.0], None),
        ([1970.0, 0.0, 1.0, 25.0, 61.0, 61.0, 1001.0], Some(((25.0 * 60.0 + 61.0) * 60.0 + 61.0) * 1000.0 + 1001.0)),
    ] {
        assert_eq!(answer(date_utc, &args.map(t)), ms.map(t), "Date.UTC{args:?}");
    }
    for (s, ms) in [
        ("2000", Some(946_684_800_000.0)),
        ("2000-02", Some(949_363_200_000.0)),
        ("2000-02-29T12:00:00Z", Some(951_825_600_000.0)),
        ("2000-02-29T12:00:00+01:00", Some(951_822_000_000.0)),
        ("2000-02-29T12:00Z", Some(951_825_600_000.0)),
        ("2000-02-29T24:00:00Z", Some(951_868_800_000.0)),
        ("2000-02-29T12:00:00", None),
        ("2000-02-30", None),
        ("2001-02-29", None),
        ("2000-13-01", None),
        ("2000-02-29T24:00:01Z", None),
        ("-000000-01-01T00:00:00Z", None),
        ("+275760-09-13T00:00:00.001Z", None),
        ("2000-02-29T12:00:00.5Z", None),
        ("Tue, 29 Feb 2000 00:00:00 GMT", None),
        ("2000-2-29", None),
    ] {
        assert_eq!(answer(date_parse, &[text(s)]), ms.map(t), "Date.parse({s:?})");
    }

    let times: Vec<f64> = vec![
        0.0,
        1.0,
        -1.0,
        999.0,
        1000.0,
        -1000.0,
        86_399_999.0,
        86_400_000.0,
        -86_400_000.0,
        -86_400_001.0,
        951_782_400_000.0,
        951_868_799_999.0,
        1_709_251_199_999.0,
        4_102_444_800_000.0,
        -62_135_596_800_000.0,
        -62_167_219_200_000.0,
        -62_198_755_200_000.0,
        1_234_567_890_123.0,
        -1_234_567_890_123.0,
        253_402_300_799_999.0,
        253_402_300_800_000.0,
        8.64e15,
        -8.64e15,
        8.64e15 - 1.0,
        -8.64e15 + 1.0,
    ];
    let time_cases: Vec<Vec<Value>> = times.iter().map(|&x| vec![t(x)]).collect();
    let now = vec![t(1_762_430_400_123.0)];

    // new Date(ms): TimeClip, over `trunc`.
    let clip_exact = function(&format!("{FLOOR}\n{TIME_CLIP}"), "time_clip", 1);
    let clip_timed = function(&standing(TIME_CLIP, &["trunc"], &one_node), "time_clip", 1);
    let clip_cases = ones([0.0, -0.0, 1.5, -1.5, -0.5, 0.5, 8.64e15, 8_640_000_000_000_002.0, -8.64e15, -8_640_000_000_000_002.0, 1e300, -1e300, 1_762_430_400_123.7].map(t).to_vec());
    scalar("new Date(ms)", &clip_exact, &clip_timed, time_clip, &clip_cases, vec![t(1_762_430_400_123.0)]);
    hot("new Date(ms)", &standing(TIME_CLIP, &["trunc"], &one_node), "time_clip", time_clip);

    // getUTC…: each part over `floor`.
    let all_parts = parts_ts();
    let getters: [(&str, &str, race::Native, String); 8] = [
        ("getUTCMilliseconds", "utc_ms", utc_ms, UTC_MS.into()),
        ("getUTCSeconds", "utc_seconds", utc_seconds, UTC_SECONDS.into()),
        ("getUTCMinutes", "utc_minutes", utc_minutes, UTC_MINUTES.into()),
        ("getUTCHours", "utc_hours", utc_hours, UTC_HOURS.into()),
        ("getUTCDay", "utc_weekday", utc_weekday, UTC_WEEKDAY.into()),
        ("getUTCFullYear", "utc_year", utc_year, civil("utc_year", "mp < 10 ? yoe + era * 400 : yoe + era * 400 + 1")),
        ("getUTCMonth", "utc_month", utc_month, civil("utc_month", "mp < 10 ? mp + 2 : mp - 10")),
        ("getUTCDate", "utc_date", utc_date, civil("utc_date", "doy - floor((153 * mp + 2) / 5) + 1")),
    ];
    for (js, name, native, src) in &getters {
        let exact = function(&format!("{FLOOR}\n{all_parts}"), name, 1);
        let stood = standing(src, &["floor"], &one_node);
        let timed = function(&stood, name, 1);
        scalar(js, &exact, &timed, *native, &time_cases, now.clone());
        hot(js, &stood, name, *native);
    }

    // Date.UTC and utc_time: MakeDay, MakeTime, MakeDate, TimeClip over `floor` and `trunc`.
    let utc_cases: Vec<Vec<Value>> = [
        [2000.0, 1.0, 29.0, 0.0, 0.0, 0.0, 0.0],
        [1999.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [2020.0, 12.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [2021.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [2021.0, -1.0, 31.0, 23.0, 59.0, 59.0, 999.0],
        [2021.0, -13.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [275_760.0, 8.0, 13.0, 0.0, 0.0, 0.0, 0.0],
        [275_760.0, 8.0, 13.0, 0.0, 0.0, 0.0, 1.0],
        [-271_821.0, 3.0, 20.0, 0.0, 0.0, 0.0, 0.0],
        [-271_821.0, 3.0, 19.0, 23.0, 59.0, 59.0, 999.0],
        [1970.0, 0.0, 1.0, 25.0, 61.0, 61.0, 1001.0],
        [1970.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
        [2000.9, 1.9, 29.9, 0.5, -0.5, 59.99, 0.9],
        [0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [-0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [99.9, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [100.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [-1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1970.0, 0.0, 100_000_001.0, 0.0, 0.0, 0.0, 0.0],
        [1970.0, 0.0, 100_000_002.0, 0.0, 0.0, 0.0, 0.0],
        [2000.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1e20],
        [1e6, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1_000_001.0, -12_000_000.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1970.0, 10_000_001.0, -304_400_000.0, 0.0, 0.0, 0.0, 0.0],
        [-1e6, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1970.0, 0.0, 1.0, 0.0, 0.0, 0.0, 4_503_599_627_370_497.0],
    ]
    .iter()
    .map(|a| a.map(t).to_vec())
    .collect();
    let utc_exact = function(&format!("{FLOOR}\n{UTC_TIME}"), "utc_time", 7);
    let utc_timed = function(&standing(UTC_TIME, &["floor", "trunc"], &one_node), "utc_time", 7);
    let some_day = [2025.0, 10.0, 6.0, 12.0, 34.0, 56.0, 789.0].map(t).to_vec();
    scalar("utc_time", &utc_exact, &utc_timed, utc_time, &utc_cases, some_day.clone());
    let date_utc_exact = function(&format!("{FLOOR}\n{UTC_TIME}\n{DATE_UTC}"), "date_utc", 7);
    let date_utc_timed = function(&standing(&standing(DATE_UTC, &["trunc"], &one_node), &["utc_time"], &one_add), "date_utc", 7);
    scalar("Date.UTC", &date_utc_exact, &date_utc_timed, date_utc, &utc_cases, some_day);

    // setUTC…: `utc_time` of the parts, one replaced.
    let setters_ts = format!("{FLOOR}\n{all_parts}\n{UTC_TIME}\n{SETTERS}");
    let setter_values = [0.0, 1.0, -1.0, 13.0, 25.0, 61.0, 1001.0, 2.7, -0.5, 1e6, 2000.0, 1e20];
    let setter_times = [0.0, 951_782_400_000.0, -62_198_755_200_000.0, 1_234_567_890_123.0, 8.64e15, -8.64e15];
    let setter_cases: Vec<Vec<Value>> = setter_times.iter().flat_map(|&a| setter_values.iter().map(move |&x| vec![t(a), t(x)])).collect();
    let setters: [(&str, &str, race::Native); 7] = [
        ("setUTCFullYear", "set_utc_full_year", set_utc_full_year),
        ("setUTCMonth", "set_utc_month", set_utc_month),
        ("setUTCDate", "set_utc_date", set_utc_date),
        ("setUTCHours", "set_utc_hours", set_utc_hours),
        ("setUTCMinutes", "set_utc_minutes", set_utc_minutes),
        ("setUTCSeconds", "set_utc_seconds", set_utc_seconds),
        ("setUTCMilliseconds", "set_utc_milliseconds", set_utc_milliseconds),
    ];
    for (js, name, native) in setters {
        let exact = function(&setters_ts, name, 2);
        check(js, &Side::Composite(&exact), &Side::Native(native), &setter_cases);
    }
    let setters_stood = standing(&standing(SETTERS, PART_NAMES, &one_node), &["utc_time"], &one_add);
    for (js, name, native) in [setters[0], setters[3]] {
        let timed = function(&setters_stood, name, 2);
        race(js, &Side::Composite(&timed), &Side::Native(native), &[1], |_| vec![t(1_762_430_400_123.0), t(7.0)]);
    }
    // In a `map`, `d.setUTCHours(7)`: the composite's function pays one `const` an item more.
    hot("setUTCHours", &format!("{setters_stood}\nfunction set_hours_7(t) {{\n  return set_utc_hours(t, 7)\n}}"), "set_hours_7", set_utc_hours_7);

    // toISOString: the parts, padded and joined.
    let iso_exact = function(&format!("{FLOOR}\n{all_parts}\n{ISO}"), "iso", 1);
    let iso_timed = function(&standing(ISO, PART_NAMES, &one_node), "iso", 1);
    check("toISOString", &Side::Composite(&iso_exact), &Side::Native(iso), &time_cases);
    race("toISOString", &Side::Composite(&iso_timed), &Side::Native(iso), &[1], |_| vec![t(1_762_430_400_123.0)]);
    hot("toISOString", &standing(ISO, PART_NAMES, &one_node), "iso", iso);
}
