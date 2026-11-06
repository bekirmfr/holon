//! Texts: each gap of JavaScript's `String.prototype`, `String.*`, the text operators, the URI
//! functions and base64, a composite against a native candidate
//! (docs/engineering/themes/js-library-races/texts.md).
//!
//! Three kinds of evidence, in order:
//!
//! 1. **The oracle.** Every native candidate, counting UTF-16 code units (texts.md, "Units"), gives
//!    what JavaScript gives on the cases in `races_texts_oracle.json`, which Node 22.21.1 (V8)
//!    computed; where JavaScript's answer has no Holon value (a lone surrogate, `NaN`, `undefined`,
//!    `null`, a throw) the candidate gives nothing.
//! 2. **The checks.** Each composite, written in TypeScript and compiled by the reader, gives what
//!    its candidate gives, twice: through today's leaves, which count Unicode scalar values, against
//!    the candidate counting scalars; and in code units, simulated (each UTF-16 code unit of the
//!    inputs made one scalar, a surrogate a private-use stand-in, and every `slice` made to give
//!    nothing when it would cut a surrogate pair, as a code-unit `slice` would), against the
//!    candidate counting code units.
//! 3. **The races.** Each composite against its candidate (counting scalars, as the composite's
//!    leaves do today), through the real `eval`.

mod race;

use std::fmt::Write as _;

use holon_core::Value;
use race::{check, function, num, race, same, text, v, Composite, Native, Side};

// ── values ───────────────────────────────────────────────────────────────────────────────────

fn text_of(v: &Value) -> Option<&str> {
    if let Value::Text(s) = v { Some(s) } else { None }
}

fn number_of(v: &Value) -> Option<f64> {
    if let Value::Number(x) = v { Some(*x) } else { None }
}

fn owned(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

fn count(k: usize) -> Value {
    Value::Number(k as f64)
}

fn truth(b: bool) -> Value {
    Value::Boolean(b)
}

/// The longest text a leaf makes, in code units: V8's bound, past which Node throws a RangeError.
const MAX_TEXT: usize = (1 << 29) - 24;

// ── units and positions ──────────────────────────────────────────────────────────────────────
//
// `W` is the unit a candidate counts in: `true`, UTF-16 code units (JavaScript's, the
// recommendation); `false`, Unicode scalar values (today's leaves').

fn width<const W: bool>(c: char) -> usize {
    if W { c.len_utf16() } else { 1 }
}

/// Code units are the scalars plus one more for each astral one.
fn len<const W: bool>(s: &str) -> usize {
    if W { s.chars().count() + astral_count(s) } else { s.chars().count() }
}

/// The astral characters: four-byte UTF-8 sequences, whose first byte is `0xF0` or above. Counted
/// in `u8`s over runs of 255 bytes, which the compiler turns into wide compares.
fn astral_count(s: &str) -> usize {
    s.as_bytes().chunks(255).map(|run| usize::from(run.iter().fold(0u8, |n, &b| n + u8::from(b >= 0xF0)))).sum()
}

/// The byte where unit `k` starts: `Ok` at a character's start (past the end, the end), `Err` with
/// the character's start when `k` falls between the halves of its surrogate pair.
fn offset<const W: bool>(s: &str, k: usize) -> Result<usize, usize> {
    let mut u = 0;
    for (b, c) in s.char_indices() {
        if u == k {
            return Ok(b);
        }
        u += width::<W>(c);
        if u > k {
            return Err(b);
        }
    }
    Ok(s.len())
}

/// Units `from..to`: empty when `to <= from`; nothing when an end cuts a surrogate pair (the
/// text would hold a lone surrogate, which a Holon text can't).
fn cut<const W: bool>(s: &str, from: usize, to: usize) -> Option<String> {
    if from >= to {
        return Some(String::new());
    }
    match (offset::<W>(s, from), offset::<W>(s, to)) {
        (Ok(a), Ok(b)) => Some(s[a..b].to_string()),
        _ => None,
    }
}

/// JavaScript's ToIntegerOrInfinity, clamped into `0..=len`.
fn clamp(x: f64, len: usize) -> usize {
    let x = x.trunc();
    if x <= 0.0 {
        0
    } else if x >= len as f64 {
        len
    } else {
        x as usize
    }
}

/// A position that counts from the end when negative, clamped: `slice`'s.
fn relative(x: f64, len: usize) -> usize {
    let x = x.trunc();
    if x < 0.0 { clamp(len as f64 + x, len) } else { clamp(x, len) }
}

/// A start clamped below only: past the end is the end, which `offset` finds.
fn start(x: f64) -> usize {
    let x = x.trunc();
    if x <= 0.0 { 0 } else { x as usize }
}

/// The first match of `t` at or after unit `k`, as a unit index. A well-formed `t` can't begin on
/// a pair's second half, so a `k` there searches from the next character.
fn find<const W: bool>(s: &str, t: &str, k: usize) -> Option<usize> {
    if t.is_empty() {
        return Some(k);
    }
    let from = match offset::<W>(s, k) {
        Ok(b) => b,
        Err(b) => b + s[b..].chars().next().map_or(0, char::len_utf8),
    };
    s[from..].find(t).map(|i| len::<W>(&s[..from + i]))
}

/// The last match of `t` beginning at or before unit `k`, as a unit index.
fn rfind<const W: bool>(s: &str, t: &str, k: usize) -> Option<usize> {
    let (Ok(b) | Err(b)) = offset::<W>(s, k);
    let mut limit = (b + t.len()).min(s.len());
    while !s.is_char_boundary(limit) {
        limit -= 1;
    }
    s[..limit].rfind(t).map(|i| len::<W>(&s[..i]))
}

// ── native candidates: what each leaf would compute ──────────────────────────────────────────

fn text_length<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    Some(count(len::<W>(text_of(s)?)))
}

/// The one unit at `k` as a text: empty past the end, nothing for half of a surrogate pair.
fn unit_at<const W: bool>(s: &str, k: usize) -> Option<String> {
    match offset::<W>(s, k) {
        Ok(b) if b == s.len() => Some(String::new()),
        Ok(b) => {
            let c = s[b..].chars().next()?;
            (width::<W>(c) == 1).then(|| c.to_string())
        }
        Err(_) => None,
    }
}

/// `s.at(i)`: from the end when negative; past either end, nothing (JavaScript's `undefined`).
fn at<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let [s, i] = xs else { return None };
    let (s, i) = (text_of(s)?, number_of(i)?.trunc());
    let k = if i < 0.0 { len::<W>(s) as f64 + i } else { i };
    if k < 0.0 {
        return None;
    }
    unit_at::<W>(s, k as usize).filter(|u| !u.is_empty()).map(Value::Text)
}

/// `s.charAt(i)`: past either end, the empty text.
fn char_at<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let [s, i] = xs else { return None };
    let i = number_of(i)?.trunc();
    if i < 0.0 {
        return Some(owned(""));
    }
    unit_at::<W>(text_of(s)?, i as usize).map(Value::Text)
}

/// `s.charCodeAt(i)`: the code unit; past either end, nothing (JavaScript's `NaN`).
fn char_code_at(xs: &[&Value]) -> Option<Value> {
    let [s, i] = xs else { return None };
    let i = number_of(i)?.trunc();
    if i < 0.0 {
        return None;
    }
    text_of(s)?.encode_utf16().nth(i as usize).map(|u| num(f64::from(u)))
}

/// `s.codePointAt(i)`: the code point a pair's first half begins, a second half's own unit;
/// past either end, nothing (`undefined`).
fn code_point_at(xs: &[&Value]) -> Option<Value> {
    let [s, i] = xs else { return None };
    let i = number_of(i)?.trunc();
    if i < 0.0 {
        return None;
    }
    let k = i as usize;
    let mut u = 0;
    for c in text_of(s)?.chars() {
        if u == k {
            return Some(num(f64::from(c as u32)));
        }
        if c.len_utf16() == 2 && u + 1 == k {
            return Some(num(f64::from(0xDC00 + ((c as u32 - 0x10000) & 0x3FF))));
        }
        u += c.len_utf16();
    }
    None
}

/// A search's inputs: the text, what is searched for, and a position when one is given.
fn search_args<'a>(xs: &[&'a Value]) -> Option<(&'a str, &'a str, Option<f64>)> {
    match xs {
        [s, t] => Some((text_of(s)?, text_of(t)?, None)),
        [s, t, p] => Some((text_of(s)?, text_of(t)?, Some(number_of(p)?))),
        _ => None,
    }
}

fn index_of<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, t, p) = search_args(xs)?;
    let k = p.map_or(0, |p| clamp(p, len::<W>(s)));
    Some(num(find::<W>(s, t, k).map_or(-1.0, |i| i as f64)))
}

fn last_index_of<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, t, p) = search_args(xs)?;
    let (n, m) = (len::<W>(s), len::<W>(t));
    if m > n {
        return Some(num(-1.0));
    }
    let k = p.map_or(n - m, |p| clamp(p, n - m));
    if t.is_empty() {
        return Some(count(k));
    }
    Some(num(rfind::<W>(s, t, k).map_or(-1.0, |i| i as f64)))
}

fn includes<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, t, p) = search_args(xs)?;
    Some(truth(find::<W>(s, t, p.map_or(0, start)).is_some()))
}

/// A well-formed `t` can't begin on a pair's second half, so only the empty text starts there.
fn starts_with<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, t, p) = search_args(xs)?;
    Some(truth(match offset::<W>(s, p.map_or(0, start)) {
        Ok(b) => s[b..].starts_with(t),
        Err(_) => t.is_empty(),
    }))
}

/// Nor end on a pair's first half.
fn ends_with<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, t, p) = search_args(xs)?;
    let end = match p {
        None => Ok(s.len()),
        Some(p) => offset::<W>(s, start(p)),
    };
    Some(truth(match end {
        Ok(b) => s[..b].ends_with(t),
        Err(_) => t.is_empty(),
    }))
}

/// A text and one or two positions, the second when given.
fn range_args<'a>(xs: &[&'a Value]) -> Option<(&'a str, f64, Option<f64>)> {
    match xs {
        [s, a] => Some((text_of(s)?, number_of(a)?, None)),
        [s, a, b] => Some((text_of(s)?, number_of(a)?, Some(number_of(b)?))),
        _ => None,
    }
}

fn js_slice<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, a, b) = range_args(xs)?;
    let n = len::<W>(s);
    cut::<W>(s, relative(a, n), b.map_or(n, |b| relative(b, n))).map(Value::Text)
}

fn substring<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, a, b) = range_args(xs)?;
    let n = len::<W>(s);
    let (x, y) = (clamp(a, n), b.map_or(n, |b| clamp(b, n)));
    cut::<W>(s, x.min(y), x.max(y)).map(Value::Text)
}

/// JavaScript's WhiteSpace and LineTerminator: not Rust's `char::is_whitespace`, which takes
/// U+0085 and leaves out U+FEFF.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

fn trim_both(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    Some(owned(text_of(s)?.trim_matches(js_space)))
}

fn trim_start(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    Some(owned(text_of(s)?.trim_start_matches(js_space)))
}

fn trim_end(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    Some(owned(text_of(s)?.trim_end_matches(js_space)))
}

/// `padStart` (`END` false) and `padEnd`: the fill repeated and cut to the units missing, which
/// can cut a pair (nothing then).
fn pad<const W: bool, const END: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, max, fill) = match xs {
        [s, m] => (text_of(s)?, number_of(m)?, " "),
        [s, m, f] => (text_of(s)?, number_of(m)?, text_of(f)?),
        _ => return None,
    };
    let n = len::<W>(s);
    let max = max.trunc();
    if max <= n as f64 || fill.is_empty() {
        return Some(owned(s));
    }
    if max > MAX_TEXT as f64 {
        return None;
    }
    let k = max as usize - n;
    let filler = cut::<W>(&fill.repeat(k.div_ceil(len::<W>(fill))), 0, k)?;
    Some(owned(if END { format!("{s}{filler}") } else { format!("{filler}{s}") }))
}

fn repeat(xs: &[&Value]) -> Option<Value> {
    let [s, c] = xs else { return None };
    let (s, c) = (text_of(s)?, number_of(c)?.trunc());
    if c < 0.0 {
        return None;
    }
    if s.is_empty() || c == 0.0 {
        return Some(owned(""));
    }
    if c * len::<true>(s) as f64 > MAX_TEXT as f64 {
        return None;
    }
    Some(owned(s.repeat(c as usize)))
}

fn to_uint32(x: f64) -> u32 {
    x.trunc().rem_euclid(4294967296.0) as u32
}

fn to_uint16(x: f64) -> u16 {
    x.trunc().rem_euclid(65536.0) as u16
}

/// `s.split(sep)`, `s.split(sep, limit)`: `""` cuts every unit (a pair's halves can't stand
/// alone: nothing); the limit is ToUint32's and keeps that many pieces.
fn split<const W: bool>(xs: &[&Value]) -> Option<Value> {
    let (s, sep, lim) = match xs {
        [s, p] => (text_of(s)?, text_of(p)?, u32::MAX),
        [s, p, l] => (text_of(s)?, text_of(p)?, to_uint32(number_of(l)?)),
        _ => return None,
    };
    let lim = lim as usize;
    let pieces: Vec<Value> = if sep.is_empty() {
        let mut out = Vec::new();
        for c in s.chars() {
            if out.len() >= lim {
                break;
            }
            if width::<W>(c) == 2 {
                return None;
            }
            out.push(owned(c.to_string()));
        }
        out
    } else {
        s.split(sep).take(lim).map(owned).collect()
    };
    Some(Value::List(pieces))
}

/// JavaScript's GetSubstitution for a text pattern (no captures): `$$`, `$&`, `` $` `` and `$'`;
/// any other `$` is itself.
fn substitute(out: &mut String, rep: &str, s: &str, at: usize, end: usize) {
    let mut rest = rep;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.as_bytes().first() {
            Some(b'$') => out.push('$'),
            Some(b'&') => out.push_str(&s[at..end]),
            Some(b'`') => out.push_str(&s[..at]),
            Some(b'\'') => out.push_str(&s[end..]),
            _ => {
                out.push('$');
                rest = after;
                continue;
            }
        }
        rest = &after[1..];
    }
    out.push_str(rest);
}

/// The same over code units, for an empty pattern that matches between a pair's halves.
fn substitute16(out: &mut Vec<u16>, rep: &[u16], s: &[u16], at: usize) {
    let unit = |c: u8| u16::from(c);
    let mut i = 0;
    while i < rep.len() {
        if rep[i] == unit(b'$') && i + 1 < rep.len() {
            let next = rep[i + 1];
            let done = if next == unit(b'$') {
                out.push(next);
                true
            } else if next == unit(b'&') {
                true
            } else if next == unit(b'`') {
                out.extend_from_slice(&s[..at]);
                true
            } else if next == unit(b'\'') {
                out.extend_from_slice(&s[at..]);
                true
            } else {
                false
            };
            if done {
                i += 2;
                continue;
            }
        }
        out.push(rep[i]);
        i += 1;
    }
}

/// `s.replace(a, b)` (`ALL` false) and `s.replaceAll(a, b)` with a text pattern.
fn replace<const W: bool, const ALL: bool>(xs: &[&Value]) -> Option<Value> {
    let [s, a, b] = xs else { return None };
    let (s, a, b) = (text_of(s)?, text_of(a)?, text_of(b)?);
    if ALL && a.is_empty() && W && s.chars().any(|c| c.len_utf16() == 2) {
        let su: Vec<u16> = s.encode_utf16().collect();
        let bu: Vec<u16> = b.encode_utf16().collect();
        let mut out = Vec::new();
        for p in 0..=su.len() {
            substitute16(&mut out, &bu, &su, p);
            if p < su.len() {
                out.push(su[p]);
            }
        }
        return String::from_utf16(&out).ok().map(owned);
    }
    let matches: Vec<usize> = match (a.is_empty(), ALL) {
        (true, false) => vec![0],
        (true, true) => s.char_indices().map(|(i, _)| i).chain([s.len()]).collect(),
        (false, true) => s.match_indices(a).map(|(i, _)| i).collect(),
        (false, false) => s.find(a).into_iter().collect(),
    };
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for at in matches {
        out.push_str(&s[last..at]);
        substitute(&mut out, b, s, at, at + a.len());
        last = at + a.len();
    }
    out.push_str(&s[last..]);
    Some(owned(out))
}

fn from_char_code(xs: &[&Value]) -> Option<Value> {
    let units = xs.iter().map(|x| number_of(x).map(to_uint16)).collect::<Option<Vec<_>>>()?;
    String::from_utf16(&units).ok().map(owned)
}

fn from_code_point(xs: &[&Value]) -> Option<Value> {
    let mut units = Vec::new();
    for x in xs {
        let x = number_of(x)?;
        if x.trunc() != x || !(0.0..=1_114_111.0).contains(&x) {
            return None;
        }
        let c = x as u32;
        if c >= 0x10000 {
            units.push(0xD800 + ((c - 0x10000) >> 10) as u16);
            units.push(0xDC00 + ((c - 0x10000) & 0x3FF) as u16);
        } else {
            units.push(c as u16);
        }
    }
    String::from_utf16(&units).ok().map(owned)
}

/// `a < b` and the rest on texts: code unit by code unit, as JavaScript compares (Rust's `str`
/// order is code points', which differs between U+E000–U+FFFF and the astral planes).
fn text_compare(xs: &[&Value]) -> Option<Value> {
    let [a, b, op] = xs else { return None };
    let o = text_of(a)?.encode_utf16().cmp(text_of(b)?.encode_utf16());
    Some(truth(match text_of(op)? {
        "<" => o.is_lt(),
        ">" => o.is_gt(),
        "<=" => o.is_le(),
        ">=" => o.is_ge(),
        _ => return None,
    }))
}

/// `encodeURIComponent` (`COMPONENT`) and `encodeURI`, which also keeps `;/?:@&=+$,#`.
fn encode_uri<const COMPONENT: bool>(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    let s = text_of(s)?;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) || (!COMPONENT && ";/?:@&=+$,#".contains(c)) {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    Some(owned(out))
}

/// `decodeURIComponent` and `decodeURI`, which leaves an escape of `;/?:@&=+$,#` as written.
/// Anything JavaScript throws a URIError on gives nothing.
fn decode_uri<const COMPONENT: bool>(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    let s = text_of(s)?;
    let bytes = s.as_bytes();
    let byte = |i: usize| -> Option<u8> {
        let d = |c: &u8| char::from(*c).to_digit(16);
        Some((d(bytes.get(i)?)? * 16 + d(bytes.get(i + 1)?)?) as u8)
    };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            let c = s[i..].chars().next()?;
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        let first = byte(i + 1)?;
        if first < 0x80 {
            let c = char::from(first);
            if !COMPONENT && ";/?:@&=+$,#".contains(c) {
                out.push_str(&s[i..i + 3]);
            } else {
                out.push(c);
            }
            i += 3;
            continue;
        }
        let n = first.leading_ones() as usize;
        if n == 1 || n > 4 {
            return None;
        }
        let mut octets = vec![first];
        let mut j = i + 3;
        for _ in 1..n {
            if bytes.get(j) != Some(&b'%') {
                return None;
            }
            octets.push(byte(j + 1)?);
            j += 3;
        }
        out.push_str(std::str::from_utf8(&octets).ok()?);
        i = j;
    }
    Some(owned(out))
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `btoa(s)`: each code unit one byte (Latin-1, not UTF-8); past U+00FF, nothing (JavaScript's
/// InvalidCharacterError).
fn btoa(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    let bytes = text_of(s)?.chars().map(|c| u8::try_from(c as u32).ok()).collect::<Option<Vec<u8>>>()?;
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16 | u32::from(*chunk.get(1).unwrap_or(&0)) << 8 | u32::from(*chunk.get(2).unwrap_or(&0));
        for k in 0..4 {
            out.push(if k <= chunk.len() { char::from(BASE64[(n >> (18 - 6 * k) & 0x3F) as usize]) } else { '=' });
        }
    }
    Some(owned(out))
}

/// `atob(s)`: WHATWG's forgiving-base64 decode (ASCII whitespace skipped, padding optional, extra
/// bits dropped), each byte one character (Latin-1).
fn atob(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    let mut data: Vec<u8> = text_of(s)?.bytes().filter(|b| !matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')).collect();
    if data.len().is_multiple_of(4) {
        if data.ends_with(b"==") {
            data.truncate(data.len() - 2);
        } else if data.ends_with(b"=") {
            data.truncate(data.len() - 1);
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0);
    for b in data {
        acc = acc << 6 | BASE64.iter().position(|&a| a == b)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(char::from((acc >> bits) as u8));
            acc &= (1 << bits) - 1;
        }
    }
    Some(owned(out))
}

/// JavaScript's Number::toString: the shortest digits that read back (Rust's `{:e}` finds them),
/// laid out with an exponent from 1e21 and below 1e-6.
fn js_number(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    if x < 0.0 {
        return format!("-{}", js_number(-x));
    }
    let e = format!("{x:e}");
    let (mantissa, exp) = e.split_once('e').expect("an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let (k, n) = (digits.len() as i32, exp.parse::<i32>().expect("a number") + 1);
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let m = if k == 1 { digits.clone() } else { format!("{}.{}", &digits[..1], &digits[1..]) };
        format!("{m}e{}{}", if n > 0 { "+" } else { "-" }, (n - 1).abs())
    }
}

/// JavaScript's ToString on the values a `+` or `String(x)` takes; a list or record gives nothing
/// (JavaScript's `"1,2"` and `"[object Object]"`: texts.md, "Differences that stay").
fn js_string(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Number(x) => Some(js_number(*x)),
        Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn js_text_of(xs: &[&Value]) -> Option<Value> {
    let [x] = xs else { return None };
    js_string(x).map(owned)
}

/// `a + b`: numbers add; with a text on either side, both as text, joined. Booleans without a
/// text give nothing (JavaScript's `true + 1` is 2: a difference that stays).
fn plus(xs: &[&Value]) -> Option<Value> {
    let [a, b] = xs else { return None };
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Some(num(x + y)),
        _ if matches!(a, Value::Text(_)) || matches!(b, Value::Text(_)) => Some(owned(js_string(a)? + &js_string(b)?)),
        _ => None,
    }
}

fn is_well_formed(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    text_of(s)?;
    Some(truth(true))
}

fn to_well_formed(xs: &[&Value]) -> Option<Value> {
    let [s] = xs else { return None };
    Some(owned(text_of(s)?))
}

// ── the oracle ───────────────────────────────────────────────────────────────────────────────

const ORACLE: &str = include_str!("races_texts_oracle.json");

/// Where a candidate gives nothing by design and JavaScript gives a value (texts.md, "Differences
/// that stay"): the method and the inputs as the oracle writes them.
const STAYS: &[(&str, &str)] = &[("+", "[true,1]")];

fn alike(a: &Option<Value>, b: &Option<Value>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => same(x, y),
        (None, None) => true,
        _ => false,
    }
}

fn oracle() {
    let table: serde_json::Value = serde_json::from_str(ORACLE).expect("the oracle's JSON");
    let mut n = 0;
    for (name, cases) in table.as_object().expect("a table") {
        let f: Native = match name.as_str() {
            "at" => at::<true>,
            "charAt" => char_at::<true>,
            "charCodeAt" => char_code_at,
            "codePointAt" => code_point_at,
            "length" => text_length::<true>,
            "slice" => js_slice::<true>,
            "substring" => substring::<true>,
            "indexOf" => index_of::<true>,
            "lastIndexOf" => last_index_of::<true>,
            "includes" => includes::<true>,
            "startsWith" => starts_with::<true>,
            "endsWith" => ends_with::<true>,
            "trim" => trim_both,
            "trimStart" => trim_start,
            "trimEnd" => trim_end,
            "padStart" => pad::<true, false>,
            "padEnd" => pad::<true, true>,
            "repeat" => repeat,
            "replace" => replace::<true, false>,
            "replaceAll" => replace::<true, true>,
            "split" => split::<true>,
            "fromCharCode" => from_char_code,
            "fromCodePoint" => from_code_point,
            "compare" => text_compare,
            "encodeURIComponent" => encode_uri::<true>,
            "encodeURI" => encode_uri::<false>,
            "decodeURIComponent" => decode_uri::<true>,
            "decodeURI" => decode_uri::<false>,
            "btoa" => btoa,
            "atob" => atob,
            "String" => js_text_of,
            "+" => plus,
            other => panic!("no candidate for {other}"),
        };
        for case in cases.as_array().expect("cases") {
            let shown = case[0].to_string();
            let inputs: Vec<Value> = case[0].as_array().expect("inputs").iter().map(|j| v(&j.to_string())).collect();
            let want = (!case[1].is_null()).then(|| v(&case[1].to_string()));
            let got = f(&inputs.iter().collect::<Vec<_>>());
            if STAYS.contains(&(name.as_str(), shown.as_str())) {
                assert!(got.is_none(), "{name}{shown}: a difference that stays gives nothing, not {got:?}");
            } else {
                assert!(alike(&got, &want), "{name}{shown}: JavaScript gives {want:?}, the candidate {got:?}");
            }
            n += 1;
        }
    }
    println!("oracle: every candidate gives JavaScript's answer, in code units, on {n} cases (Node 22.21.1)");
}

// ── code units, simulated ────────────────────────────────────────────────────────────────────

/// Each UTF-16 code unit of `s` as one scalar, a surrogate as a private-use stand-in (U+F0000 plus
/// its offset from U+D800), so leaves that count scalars count code units.
fn widen(s: &str) -> String {
    s.encode_utf16()
        .map(|u| char::from_u32(if (0xD800..0xE000).contains(&u) { 0xF0000 + (u32::from(u) - 0xD800) } else { u32::from(u) }).expect("a scalar"))
        .collect()
}

/// `widen` undone; nothing when a stand-in has lost its other half.
fn narrow(s: &str) -> Option<String> {
    let mut units = Vec::new();
    for c in s.chars() {
        let x = c as u32;
        if (0xF0000..0xF0800).contains(&x) {
            units.push((x - 0xF0000 + 0xD800) as u16);
        } else {
            let mut b = [0u16; 2];
            units.extend_from_slice(c.encode_utf16(&mut b));
        }
    }
    String::from_utf16(&units).ok()
}

fn widen_value(x: &Value) -> Value {
    match x {
        Value::Text(s) => Value::Text(widen(s)),
        Value::List(xs) => Value::List(xs.iter().map(widen_value).collect()),
        x => x.clone(),
    }
}

fn narrow_value(x: Value) -> Option<Value> {
    match x {
        Value::Text(s) => narrow(&s).map(Value::Text),
        Value::List(xs) => xs.into_iter().map(narrow_value).collect::<Option<Vec<_>>>().map(Value::List),
        x => Some(x),
    }
}

/// A composite's text with each call of `slice` made strict: nothing when the piece would begin on
/// a pair's second half or end on its first, as a `slice` counting code units gives. A stand-in
/// for a first half is U+F0000–U+F03FF, UTF-8 `f3b08…`; for a second half U+F0400–U+F07FF,
/// `f3b09…`.
fn strictly(src: &str) -> String {
    let imported = |leaf: &str| src.lines().any(|l| l.starts_with("import ") && l.contains(leaf));
    let missing: Vec<&str> = ["slice", "text_length", "hex"].into_iter().filter(|l| !imported(l)).collect();
    let mut out = if missing.is_empty() { String::new() } else { format!("import {{ {} }} from \"holon\"\n", missing.join(", ")) };
    let mut rest = src;
    while let Some(i) = rest.find("slice(") {
        let named = rest[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        out.push_str(&rest[..i]);
        out.push_str(if named { "slice(" } else { "strict_slice(" });
        rest = &rest[i + "slice(".len()..];
    }
    out.push_str(rest);
    out.push_str(
        r#"

function strict_slice(s, a, l) {
  const r = slice(s, a, l)
  const n = text_length(r)
  const whole = n === 0 ? true : (slice(hex(slice(r, 0, 1)), 0, 5) !== "f3b09" && slice(hex(slice(r, n - 1, 1)), 0, 5) !== "f3b08")
  return whole ? r : undefined
}"#,
    );
    out
}

/// The composite `name` of `src` gives what `native` (counting code units) gives on every case, in
/// simulated code units.
fn check_units(name: &str, src: &str, function_name: &str, arity: usize, native: Native, cases: &[Vec<Value>]) {
    let c = function(&strictly(src), function_name, arity);
    for case in cases {
        let want = native(&case.iter().collect::<Vec<_>>());
        let wide: Vec<Value> = case.iter().map(widen_value).collect();
        let got = c.call(&wide).and_then(narrow_value);
        assert!(alike(&got, &want), "{name}, in code units: the composite gives {got:?} and the candidate {want:?} on {case:?}");
    }
}

/// Where a composite is not exact: it and the candidate differ on `case` (a claim of texts.md, kept
/// true). `units` runs it in simulated code units.
fn differs(name: &str, c: &Composite, native: Native, case: &[Value], units: bool) {
    let want = native(&case.iter().collect::<Vec<_>>());
    let got = if units { c.call(&case.iter().map(widen_value).collect::<Vec<_>>()).and_then(narrow_value) } else { c.call(case) };
    assert!(!alike(&got, &want), "{name}: said to differ on {case:?}, but both give {got:?}");
    println!("{name}: inexact, as texts.md says: on {case:?} the composite gives {got:?}, JavaScript {want:?}");
}

// ── the composites, as raced ─────────────────────────────────────────────────────────────────

const AT: &str = r#"import { text_length, slice } from "holon"
function js_at(s, i) {
  const n = text_length(s)
  const k = i < 0 ? n + i : i
  return k >= 0 && k < n ? slice(s, k, 1) : undefined
}"#;

const CHAR_AT: &str = r#"import { text_length, slice } from "holon"
function js_char_at(s, i) {
  return slice(s, i < 0 ? 0 : i, i < 0 ? 0 : 1)
}"#;

const SLICE: &str = r#"import { text_length, slice } from "holon"
function js_slice(s, a, b) {
  const n = text_length(s)
  const lo = a < 0 ? (n + a < 0 ? 0 : n + a) : a
  const hi = b < 0 ? (n + b < 0 ? 0 : n + b) : (b > n ? n : b)
  return slice(s, lo, hi > lo ? hi - lo : 0)
}"#;

const SUBSTRING: &str = r#"import { text_length, slice } from "holon"
function js_substring(s, a, b) {
  const n = text_length(s)
  const x = a < 0 ? 0 : (a > n ? n : a)
  const y = b < 0 ? 0 : (b > n ? n : b)
  return slice(s, x < y ? x : y, x < y ? y - x : x - y)
}"#;

const INCLUDES: &str = r#"function js_includes(s, t) {
  return s.indexOf(t) !== -1
}"#;

const STARTS_WITH: &str = r#"import { text_length, slice } from "holon"
function js_starts_with(s, t) {
  return (slice(s, 0, text_length(t)) === t) ?? false
}"#;

const STARTS_WITH_AT: &str = r#"import { text_length, slice } from "holon"
function js_starts_with_at(s, t, p) {
  return (slice(s, p < 0 ? 0 : p, text_length(t)) === t) ?? false
}"#;

const ENDS_WITH: &str = r#"import { text_length, slice } from "holon"
function js_ends_with(s, t) {
  const n = text_length(s)
  const m = text_length(t)
  return n >= m ? (slice(s, n - m, m) === t) ?? false : false
}"#;

const INDEX_FROM: &str = r#"import { text_length, slice } from "holon"
function js_index_from(s, t, p) {
  const n = text_length(s)
  const k = p < 0 ? 0 : (p > n ? n : p)
  const rest = slice(s, k, n) ?? slice(s, k + 1, n)
  const i = rest.indexOf(t)
  return t === "" ? k : (i === -1 ? -1 : i + n - text_length(rest))
}"#;

const LAST_INDEX_OF: &str = r#"import { text_length, slice, range } from "holon"
function js_last_index_of(s, t) {
  const n = text_length(s)
  const m = text_length(t)
  return m > n ? -1 : range(n - m + 1).reduce((acc, i) => ((slice(s, i, m) === t) ?? false ? i : acc), -1)
}"#;

const TRIM_START: &str = r#"import { text_length, slice, range } from "holon"
function js_trim_start(s) {
  const n = text_length(s)
  const k = range(n).reduce((acc, i) => (acc === i && (" \t\n\v\f\r                 　﻿".indexOf(slice(s, i, 1)) ?? -1) !== -1 ? i + 1 : acc), 0)
  return slice(s, k, n)
}"#;

const REPEAT: &str = r#"import { range } from "holon"
function js_repeat(s, k) {
  return range(k).map((i) => s ?? i).join("")
}"#;

const PAD_START: &str = r#"import { text_length, slice, range } from "holon"
function js_pad_start(s, len, fill) {
  const k = len - text_length(s)
  const m = k > 0 && fill !== "" ? k : 0
  return `${slice(range(m).map((i) => fill ?? i).join(""), 0, m)}${s}`
}"#;

// `repeat` as a leaf, stood in for: `fills` is the fill already repeated, and the inner `slice`
// takes `repeat`'s place (one node, the same copy of `k` units for a one-unit fill).
const PAD_START_PROXY: &str = r#"import { text_length, slice } from "holon"
function js_pad_start_proxy(s, len, fills) {
  const k = len - text_length(s)
  const m = k > 0 && fills !== "" ? k : 0
  return `${slice(slice(fills, 0, m), 0, m)}${s}`
}"#;

const SPLIT_UNITS: &str = r#"import { text_length, slice, range } from "holon"
function js_split_units(s) {
  return range(text_length(s)).map((i) => slice(s, i, 1))
}"#;

const SPLIT_LIMIT: &str = r#"import { length, range, get } from "holon"
function js_split_limit(s, sep, lim) {
  const parts = s.split(sep)
  return lim >= length(parts) ? parts : range(lim).map((i) => get(parts, i))
}"#;

const REPLACE_ALL: &str = r#"function js_replace_all(s, a, b) {
  return s.split(a).join(b)
}"#;

const REPLACE: &str = r#"import { text_length, slice } from "holon"
function js_replace(s, a, b) {
  const i = s.indexOf(a)
  return i === -1 ? s : `${slice(s, 0, i)}${b}${slice(s, i + text_length(a), text_length(s))}`
}"#;

const IS_WELL_FORMED: &str = r#"import { text_length, slice } from "holon"
function js_is_well_formed(s) {
  return slice(s, 0, 0) === ""
}"#;

// `concat` of the text and "": the text for a text, nothing for anything else (JavaScript's
// TypeError), as the reader would lower `s.toWellFormed()`.
const TO_WELL_FORMED: &str = r#"import { concat } from "holon"
function js_to_well_formed(s) {
  return concat(s, "")
}"#;

const PLUS: &str = r#"import { json_stringify, slice, text_length } from "holon"
function kind(x) {
  return slice(json_stringify(x), 0, 1)
}
function js_plus(a, b) {
  const ka = kind(a)
  const kb = kind(b)
  const listy = ka === "[" || ka === "{" || kb === "[" || kb === "{"
  return !listy ? (ka === '"' || kb === '"' ? `${a}${b}` : a + b) : undefined
}"#;

const ENCODE_COMPONENT: &str = r#"import { text_length, slice, range, hex, uppercase } from "holon"
function pct(h) {
  const n = text_length(h)
  return `%${slice(h, 0, 2)}${n > 2 ? `%${slice(h, 2, 2)}` : ""}${n > 4 ? `%${slice(h, 4, 2)}` : ""}${n > 6 ? `%${slice(h, 6, 2)}` : ""}`
}
function enc(c) {
  return "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()".indexOf(c) !== -1 ? c : pct(uppercase(hex(c)))
}
function js_encode_component(s) {
  return range(text_length(s)).map((i) => enc(slice(s, i, 1))).join("")
}"#;

const DECODE_COMPONENT: &str = r#"import { text_length, slice, hex, head, tail } from "holon"
function js_decode_component(s) {
  const parts = s.split("%")
  const rest = tail(parts).map((p) => (text_length(p) >= 2 ? `${slice(p, 0, 2)}${hex(slice(p, 2, text_length(p)))}` : undefined))
  return hex(`${hex(head(parts))}${rest.join("")}`, { mode: "decode" })
}"#;

// ── cases and inputs ─────────────────────────────────────────────────────────────────────────

/// Texts every composite is checked on: empty, one character, astral characters (one alone, two
/// together, between others), a combining mark, spaces at both ends.
const TEXTS: &[&str] = &["", "a", "abc", "a\u{1F600}b", "\u{1F600}\u{1F600}", "e\u{301}x", " a "];
/// Searched texts: matches first and last, overlapping, astral.
const HAYSTACKS: &[&str] = &["", "a", "aaa", "abcabc", "a\u{1F600}b\u{1F600}", "x\u{1F600}"];
const NEEDLES: &[&str] = &["", "a", "aa", "bc", "\u{1F600}", "z", "abcabcd"];
const POSITIONS: &[f64] = &[0.0, 1.0, 2.0, 3.0, -1.0, -2.0, -9.0, 9.0];

fn cases1(texts: &[&str]) -> Vec<Vec<Value>> {
    texts.iter().map(|s| vec![text(s)]).collect()
}

fn cases_at(texts: &[&str], positions: &[f64]) -> Vec<Vec<Value>> {
    texts.iter().flat_map(|s| positions.iter().map(move |&p| vec![text(s), num(p)])).collect()
}

fn cases_range(texts: &[&str]) -> Vec<Vec<Value>> {
    let ends = [0.0, 1.0, 2.0, 3.0, -1.0, -2.0, 9.0];
    texts.iter().flat_map(|s| [0.0, 1.0, 2.0, -1.0, -3.0, 9.0].into_iter().flat_map(move |a| ends.into_iter().map(move |b| vec![text(s), num(a), num(b)]))).collect()
}

fn cases_search() -> Vec<Vec<Value>> {
    HAYSTACKS.iter().flat_map(|s| NEEDLES.iter().map(move |t| vec![text(s), text(t)])).collect()
}

fn cases_search_at() -> Vec<Vec<Value>> {
    cases_search().into_iter().flat_map(|c| [0.0, 1.0, 2.0, 3.0, -1.0, 9.0].into_iter().map(move |p| vec![c[0].clone(), c[1].clone(), num(p)])).collect()
}

/// `n` characters of a sentence, mostly ASCII, an `é` among them so the walk meets a two-byte
/// character.
fn sample(n: usize) -> String {
    "The quick brown fox jumps over the lazy dog, café! ".chars().cycle().take(n).collect()
}

fn sized(n: usize) -> Vec<Value> {
    vec![text(&sample(n))]
}

/// Check a composite both ways (today's leaves; simulated code units), and return it for its race.
fn checked(name: &str, src: &str, f: &str, arity: usize, scalars: Native, units: Native, cases: &[Vec<Value>]) -> Composite {
    let c = function(src, f, arity);
    check(name, &Side::Composite(&c), &Side::Native(scalars), cases);
    check_units(name, src, f, arity, units, cases);
    c
}

const SIZES: &[usize] = &[8, 64, 4096];
/// For a composite that grows faster than its candidate: kept small.
const SMALL: &[usize] = &[8, 64, 512];

// ── where hot: a composite prepared once and applied to each of 16 texts, as `map` applies one ──
//
// A composite called on its own pays a floor a leaf doesn't (about 470 ns today: one leaf in a
// composite, `s ?? ""`, costs 535 ns where the leaf behind `eval` costs 62 ns). Mapped by name,
// `xs.map(f)`, its body is prepared once for the list, which shows what each application costs.
// (The harness's `mapped` writes `xs.map((x) => f(x))`, whose every application calls `f` as a
// nested composite and pays the floor again.)

const ITEMS: usize = 16;

fn texts_of(n: usize) -> Vec<Value> {
    vec![Value::List((0..ITEMS).map(|_| text(&sample(n))).collect())]
}

fn zq() -> &'static Value {
    static V: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    V.get_or_init(|| text("zq"))
}

fn the() -> &'static Value {
    static V: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    V.get_or_init(|| text("The "))
}

fn at_3(xs: &[&Value]) -> Option<Value> {
    at::<false>(&[xs[0], &num(3.0)])
}

fn char_at_3(xs: &[&Value]) -> Option<Value> {
    char_at::<false>(&[xs[0], &num(3.0)])
}

fn slice_1(xs: &[&Value]) -> Option<Value> {
    js_slice::<false>(&[xs[0], &num(1.0), &num(-1.0)])
}

fn includes_zq(xs: &[&Value]) -> Option<Value> {
    includes::<false>(&[xs[0], zq()])
}

fn starts_the(xs: &[&Value]) -> Option<Value> {
    starts_with::<false>(&[xs[0], the()])
}

fn is_well_formed_2(xs: &[&Value]) -> Option<Value> {
    is_well_formed(&xs[..1])
}

/// A composite made unary: `header` (its `function` line) replaced by `unary`'s, the inputs past
/// the first bound by `const` lines, so the body is the same graph. Mapped by name, checked
/// against the mapped candidate, and raced.
fn mapped_race(name: &str, src: &str, header: &str, unary: &str, binds: &str, native: Native) {
    assert!(src.contains(header), "{header} isn't in the composite");
    let body = src.replace(header, &format!("function {unary}(s) {{{binds}"));
    let m = function(&format!("{body}\n\nfunction each_{unary}(xs) {{\n  return xs.map({unary})\n}}"), &format!("each_{unary}"), 1);
    let items = vec![Value::List(["abcdef", "a\u{1F600}bcd", "The quick", "  x  "].iter().map(|s| text(s)).collect())];
    check(name, &Side::Composite(&m), &Side::NativeMapped(native), &[items]);
    race(name, &Side::Composite(&m), &Side::NativeMapped(native), SIZES, texts_of);
}

// A lower bound for `as_text` writing numbers as JavaScript does, composed: the test for the
// exponent's ranges alone, before any reformatting (which needs folds over the digits).
const NUMBER_TEXT_FLOOR: &str = r#"function js_number_text(x) {
  const a = x < 0 ? 0 - x : x
  return a >= 1e21 || (a < 0.000001 && a !== 0) ? `${x}e` : `${x}`
}"#;

/// What the reader does today that a composite has to write around: `c ? a : b` lowers to
/// `guard(c, a) ?? b`, so a chosen `a` that is absent gives `b`, where JavaScript gives
/// `undefined` (texts.md, "Reader changes").
fn reader_facts() {
    let c = function("function pick(c, r) {\n  return c ? r.missing : \"other\"\n}", "pick", 2);
    assert_eq!(c.call(&[truth(true), v("{}")]), Some(text("other")), "the ternary's fall-through is fixed: update texts.md");
    println!("reader: `c ? a : b` with c true and a absent gives b (JavaScript gives undefined)");
}

fn main() {
    oracle();
    reader_facts();

    // What counting code units costs the leaves that count: the same pass as counting scalars,
    // on a text with an astral character and a two-byte one in every 30.
    let astral = |n: usize| -> String { "The quick \u{1F600} brown fox, café! ".chars().cycle().take(n).collect() };
    race("text_length, code units (left) against scalars (right)", &Side::Native(text_length::<true>), &Side::Native(text_length::<false>), SIZES, |n| vec![text(&astral(n))]);
    race("s.slice(1, -1), code units (left) against scalars (right)", &Side::Native(js_slice::<true>), &Side::Native(js_slice::<false>), SIZES, |n| vec![text(&astral(n)), num(1.0), num(-1.0)]);
    race("s.indexOf(t), t absent, code units (left) against scalars (right)", &Side::Native(index_of::<true>), &Side::Native(index_of::<false>), SIZES, |n| vec![text(&astral(n)), text("zq")]);

    // s.at(i), s.charAt(i): a slice of one unit, around a comparison or two.
    let c = checked("at", AT, "js_at", 2, at::<false>, at::<true>, &cases_at(TEXTS, POSITIONS));
    race("s.at(i)", &Side::Composite(&c), &Side::Native(at::<false>), SIZES, |n| vec![text(&sample(n)), count(n / 2)]);
    mapped_race("s.at(3), mapped over 16 texts", AT, "function js_at(s, i) {", "at_3", "\n  const i = 3", at_3);
    let c = checked("charAt", CHAR_AT, "js_char_at", 2, char_at::<false>, char_at::<true>, &cases_at(TEXTS, POSITIONS));
    race("s.charAt(i)", &Side::Composite(&c), &Side::Native(char_at::<false>), SIZES, |n| vec![text(&sample(n)), count(n / 2)]);
    mapped_race("s.charAt(3), mapped over 16 texts", CHAR_AT, "function js_char_at(s, i) {", "char_at_3", "\n  const i = 3", char_at_3);

    // s.slice(a, b), s.substring(a, b).
    let c = checked("slice", SLICE, "js_slice", 3, js_slice::<false>, js_slice::<true>, &cases_range(TEXTS));
    race("s.slice(a, b)", &Side::Composite(&c), &Side::Native(js_slice::<false>), SIZES, |n| vec![text(&sample(n)), num(1.0), num(-1.0)]);
    mapped_race("s.slice(1, -1), mapped over 16 texts", SLICE, "function js_slice(s, a, b) {", "slice_1", "\n  const a = 1\n  const b = -1", slice_1);
    let c = checked("substring", SUBSTRING, "js_substring", 3, substring::<false>, substring::<true>, &cases_range(TEXTS));
    race("s.substring(a, b)", &Side::Composite(&c), &Side::Native(substring::<false>), SIZES, |n| vec![text(&sample(n)), count(n / 4), count(3 * n / 4)]);

    // Searches: includes, startsWith, endsWith, indexOf from a position, lastIndexOf.
    let c = checked("includes", INCLUDES, "js_includes", 2, includes::<false>, includes::<true>, &cases_search());
    race("s.includes(t), t absent", &Side::Composite(&c), &Side::Native(includes::<false>), SIZES, |n| vec![text(&sample(n)), text("zq")]);
    mapped_race("s.includes(\"zq\"), mapped over 16 texts", INCLUDES, "function js_includes(s, t) {", "includes_zq", "\n  const t = \"zq\"", includes_zq);
    let c = checked("startsWith", STARTS_WITH, "js_starts_with", 2, starts_with::<false>, starts_with::<true>, &cases_search());
    race("s.startsWith(t)", &Side::Composite(&c), &Side::Native(starts_with::<false>), &[8, 64, 4096, 65536], |n| vec![text(&sample(n)), text("The ")]);
    mapped_race("s.startsWith(\"The \"), mapped over 16 texts", STARTS_WITH, "function js_starts_with(s, t) {", "starts_the", "\n  const t = \"The \"", starts_the);
    checked("startsWith at", STARTS_WITH_AT, "js_starts_with_at", 3, starts_with::<false>, starts_with::<true>, &cases_search_at());
    let c = checked("endsWith", ENDS_WITH, "js_ends_with", 2, ends_with::<false>, ends_with::<true>, &cases_search());
    race("s.endsWith(t), t of 4", &Side::Composite(&c), &Side::Native(ends_with::<false>), &[8, 64, 4096, 65536], |n| {
        let s = sample(n);
        let t: String = s.chars().skip(n - 4).collect();
        vec![text(&s), text(&t)]
    });
    let c = checked("indexOf at", INDEX_FROM, "js_index_from", 3, index_of::<false>, index_of::<true>, &cases_search_at());
    race("s.indexOf(t, p), t absent", &Side::Composite(&c), &Side::Native(index_of::<false>), SIZES, |n| vec![text(&sample(n)), text("zq"), count(n / 2)]);
    let c = checked("lastIndexOf", LAST_INDEX_OF, "js_last_index_of", 2, last_index_of::<false>, last_index_of::<true>, &cases_search());
    race("s.lastIndexOf(t)", &Side::Composite(&c), &Side::Native(last_index_of::<false>), SMALL, |n| vec![text(&sample(n)), text("The")]);

    // trimStart, for the trim family: a fold over the characters.
    let spaces = ["", "a", " a ", " \t\n\u{B}\u{C}\r\u{A0}\u{1680}\u{2000}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}x y ", "\u{85}x", "\u{FEFF}\u{FEFF}", "\u{1F600} ", "   "];
    let c = checked("trimStart", TRIM_START, "js_trim_start", 1, trim_start, trim_start, &cases1(&spaces));
    race("s.trimStart(), 4 spaces first", &Side::Composite(&c), &Side::Native(trim_start), SMALL, |n| vec![text(&format!("    {}", sample(n - 4)))]);

    // repeat, then padStart over it.
    let reps: Vec<Vec<Value>> = ["", "ab", "\u{1F600}"].iter().flat_map(|s| [0.0, 1.0, 3.0].map(|k| vec![text(s), num(k)])).collect();
    let c = checked("repeat", REPEAT, "js_repeat", 2, repeat, repeat, &reps);
    race("s.repeat(k), s of 2", &Side::Composite(&c), &Side::Native(repeat), SIZES, |n| vec![text("ab"), count(n)]);
    let pads: Vec<Vec<Value>> = ["", "abc", "\u{1F600}"]
        .iter()
        .flat_map(|s| [0.0, 2.0, 5.0, -1.0].into_iter().flat_map(move |l| ["", "0", "ab", "\u{1F600}"].map(|f| vec![text(s), num(l), text(f)])))
        .collect();
    let c = checked("padStart", PAD_START, "js_pad_start", 3, pad::<false, false>, pad::<true, false>, &pads);
    race("s.padStart(len, \"0\"), half filled", &Side::Composite(&c), &Side::Native(pad::<false, false>), SIZES, |n| vec![text(&sample(n / 2)), count(n), text("0")]);
    let fills: Vec<Vec<Value>> = ["", "abc", "\u{1F600}"]
        .iter()
        .flat_map(|s| [0.0, 2.0, 5.0, -1.0].into_iter().flat_map(move |l| ["", "0000000", "ababab", "\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}"].map(|f| vec![text(s), num(l), text(f)])))
        .collect();
    let c = checked("padStart, repeat a leaf", PAD_START_PROXY, "js_pad_start_proxy", 3, pad::<false, false>, pad::<true, false>, &fills);
    race("s.padStart(len, \"0\"), repeat as a leaf (stand-in)", &Side::Composite(&c), &Side::Native(pad::<false, false>), SIZES, |n| vec![text(&sample(n / 2)), count(n), text(&"0".repeat(n))]);

    // split: "" and a limit.
    let c = function(SPLIT_UNITS, "js_split_units", 1);
    let split_all = |xs: &[&Value]| -> Option<Value> { split::<false>(&[xs[0], &text("")]) };
    let split_units = |xs: &[&Value]| -> Option<Value> { split::<true>(&[xs[0], &text("")]) };
    check("split(\"\")", &Side::Composite(&c), &Side::Native(split_all), &cases1(TEXTS));
    check_units("split(\"\")", SPLIT_UNITS, "js_split_units", 1, split_units, &cases1(TEXTS));
    race("s.split(\"\")", &Side::Composite(&c), &Side::Native(split_all), SMALL, sized);
    let limits: Vec<Vec<Value>> = ["", "a,b,,c,", "abc", "a\u{1F600}b"]
        .iter()
        .flat_map(|s| [",", "b"].into_iter().flat_map(move |p| [0.0, 1.0, 2.0, 10.0].map(|l| vec![text(s), text(p), num(l)])))
        .collect();
    let c = function(SPLIT_LIMIT, "js_split_limit", 3);
    check("split limit", &Side::Composite(&c), &Side::Native(split::<false>), &limits);
    check_units("split limit", SPLIT_LIMIT, "js_split_limit", 3, split::<true>, &limits);
    differs("split limit", &c, split::<false>, &[text("a,b"), text(","), num(-1.0)], false);
    race("s.split(\" \", 2)", &Side::Composite(&c), &Side::Native(split::<false>), SIZES, |n| vec![text(&sample(n)), text(" "), num(2.0)]);

    // replaceAll and replace with texts: exact while the replacement has no `$`.
    let swaps: Vec<Vec<Value>> = ["", "abc", "abcabc", "aaa", "a\u{1F600}b"]
        .iter()
        .flat_map(|s| ["a", "aa", "b", "z", "\u{1F600}"].into_iter().flat_map(move |a| ["", "-", "xy"].map(|b| vec![text(s), text(a), text(b)])))
        .collect();
    let c = function(REPLACE_ALL, "js_replace_all", 3);
    check("replaceAll", &Side::Composite(&c), &Side::Native(replace::<false, true>), &swaps);
    check_units("replaceAll", REPLACE_ALL, "js_replace_all", 3, replace::<true, true>, &swaps);
    differs("replaceAll", &c, replace::<false, true>, &[text("abc"), text("b"), text("$&$&")], false);
    differs("replaceAll", &c, replace::<false, true>, &[text("abc"), text(""), text("-")], false);
    race("s.replaceAll(\" \", \"_\")", &Side::Composite(&c), &Side::Native(replace::<false, true>), SIZES, |n| vec![text(&sample(n)), text(" "), text("_")]);
    let mut firsts = swaps.clone();
    firsts.extend(["", "abc", "a\u{1F600}b"].map(|s| vec![text(s), text(""), text("-")]));
    let c = checked("replace", REPLACE, "js_replace", 3, replace::<false, false>, replace::<true, false>, &firsts);
    differs("replace", &c, replace::<false, false>, &[text("abc"), text("b"), text("[$`]")], false);
    race("s.replace(\"e\", \"E\")", &Side::Composite(&c), &Side::Native(replace::<false, false>), SIZES, |n| vec![text(&sample(n)), text("e"), text("E")]);

    // isWellFormed, toWellFormed: true, and the text itself.
    let c = checked("isWellFormed", IS_WELL_FORMED, "js_is_well_formed", 1, is_well_formed, is_well_formed, &cases1(TEXTS));
    check("isWellFormed, not a text", &Side::Composite(&c), &Side::Native(is_well_formed), &[vec![num(1.0)], vec![v("[1]")]]);
    race("s.isWellFormed()", &Side::Composite(&c), &Side::Native(is_well_formed), SIZES, sized);
    mapped_race("s.isWellFormed(), mapped over 16 texts", IS_WELL_FORMED, "function js_is_well_formed(s) {", "well_formed", "", is_well_formed);
    let equals = race::leaf("equals", &[]);
    check("isWellFormed as s === s", &Side::Leaf(&equals), &Side::Native(is_well_formed_2), &cases1(TEXTS).into_iter().map(|c| vec![c[0].clone(), c[0].clone()]).collect::<Vec<_>>());
    race("s.isWellFormed() as s === s, the leaf equals", &Side::Leaf(&equals), &Side::Native(is_well_formed_2), SIZES, |n| vec![text(&sample(n)), text(&sample(n))]);
    let c = function(TO_WELL_FORMED, "js_to_well_formed", 1);
    check("toWellFormed", &Side::Composite(&c), &Side::Native(to_well_formed), &cases1(TEXTS));
    check("toWellFormed, not a text", &Side::Composite(&c), &Side::Native(to_well_formed), &[vec![num(1.0)], vec![v("[\"a\"]")], vec![truth(true)]]);
    race("s.toWellFormed()", &Side::Composite(&c), &Side::Native(to_well_formed), SIZES, sized);
    mapped_race("s.toWellFormed(), mapped over 16 texts", TO_WELL_FORMED, "function js_to_well_formed(s) {", "well_formed_text", "", to_well_formed);

    // `${x}` and String(x) on a number: the composite's test alone already loses.
    let floor = function(&format!("{NUMBER_TEXT_FLOOR}\n\nfunction each(xs) {{\n  return xs.map(js_number_text)\n}}"), "each", 1);
    race("String(x), JavaScript's layout: the composite's test alone (a lower bound), mapped", &Side::Composite(&floor), &Side::NativeMapped(js_text_of), &[1, 64, 4096], |n| vec![race::numbers(n)]);

    // a + b: a type test through json_stringify, then concat or add.
    let pairs: Vec<Vec<Value>> = vec![
        vec![text("a"), text("b")],
        vec![text("a"), num(1.0)],
        vec![num(1.0), text("a")],
        vec![num(1.0), num(2.0)],
        vec![truth(true), text("a")],
        vec![text("a"), truth(false)],
        vec![num(1.5), text("x")],
        vec![truth(true), num(1.0)],
        vec![v("[1,2]"), text("a")],
        vec![text("a"), v("{\"a\":1}")],
        vec![text("\u{1F600}"), text("e\u{301}")],
    ];
    let c = checked("+", PLUS, "js_plus", 2, plus, plus, &pairs);
    differs("+", &c, plus, &[num(1e21), text("")], false);
    // Not a text's: `add` folds from 0, so -0 + -0 gives 0 (numbers.md's to settle).
    differs("+", &c, plus, &[num(-0.0), num(-0.0)], false);
    race("a + b, two texts of n/2", &Side::Composite(&c), &Side::Native(plus), SIZES, |n| vec![text(&sample(n / 2)), text(&sample(n / 2))]);

    // encodeURIComponent: a character at a time; decodeURIComponent: one hex decode.
    let uris = ["", "abc", "a b&c=d/e?f#g", "\u{E9}", "\u{1F600}", "-_.!~*'()", "100%"];
    let c = function(ENCODE_COMPONENT, "js_encode_component", 1);
    check("encodeURIComponent", &Side::Composite(&c), &Side::Native(encode_uri::<true>), &cases1(&uris));
    differs("encodeURIComponent", &function(&strictly(ENCODE_COMPONENT), "js_encode_component", 1), encode_uri::<true>, &[text("a\u{1F600}")], true);
    race("encodeURIComponent(s)", &Side::Composite(&c), &Side::Native(encode_uri::<true>), SMALL, sized);
    let escapes = [
        "", "abc", "a%20b", "%C3%A9%F0%9F%98%80", "%c3%a9", "%2F%3f%23%25", "%", "%4", "%4g", "%C3", "%C3%28", "%C0%80", "%ED%A0%80", "%F4%90%80%80", "%E2%82", "\u{E9}%41", "%4%1", "\u{1F600}%41", "%4\u{1F600}",
    ];
    let c = checked("decodeURIComponent", DECODE_COMPONENT, "js_decode_component", 1, decode_uri::<true>, decode_uri::<true>, &cases1(&escapes));
    race("decodeURIComponent(s), n characters encoded", &Side::Composite(&c), &Side::Native(decode_uri::<true>), SIZES, |n| {
        vec![encode_uri::<true>(&[&text(&sample(n))]).expect("encoded")]
    });
}
