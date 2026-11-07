//! The text leaves: JavaScript's string methods, its text operators, the URI functions and
//! `btoa`/`atob` on Holon's texts (docs/engineering/themes/js-library-races/texts.md, "Leaf
//! specs"). Each was a native candidate in the races (`crates/holon-author/benches/races_texts.rs`),
//! checked there against what Node computed (`races_texts_oracle.json`), which the tests here read
//! too.
//!
//! Positions and lengths are UTF-16 code units, as JavaScript counts them: a character above
//! U+FFFF (an emoji, say) is two, a surrogate pair. A Holon text is UTF-8 and can't hold a lone
//! surrogate, so a result that would begin or end inside a pair gives nothing, where JavaScript
//! gives a text holding half of it. A position is truncated toward zero (ToIntegerOrInfinity); a
//! text padded, repeated or replaced past [`MAX_TEXT`] units gives nothing, where Node makes it up
//! to 32 times longer, then throws. None computes a float
//! beyond truncating and counting, so each gives the same natively and in wasm. `normalize` reads
//! `unicode-normalization`'s tables, pinned.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Write as _;

use unicode_normalization::UnicodeNormalization;

use crate::lists::relative;
use crate::Value;

/// The longest text `pad`, `repeat`, `replace` and a laid-out `json_stringify` make, in code
/// units: 16 Mi, twice the longest body a request brings, so a width or a count from a request
/// makes a leaf allocate tens of megabytes at most, not gigabytes. V8's bound, past which Node
/// throws a RangeError, is 32 times this (2^29 − 24); one server holds many flows' runs at once.
pub(crate) const MAX_TEXT: usize = 1 << 24;

// ── units and positions ──────────────────────────────────────────────────────────────────────

/// A text's length in code units: its characters, plus one more for each astral one.
pub(crate) fn units(s: &str) -> usize {
    s.chars().count() + astral_count(s)
}

/// The astral characters: four-byte UTF-8 sequences, whose first byte is `0xF0` or above. Counted
/// in `u8`s over runs of 255 bytes, which the compiler turns into wide compares.
fn astral_count(s: &str) -> usize {
    s.as_bytes().chunks(255).map(|run| usize::from(run.iter().fold(0u8, |n, &b| n + u8::from(b >= 0xF0)))).sum()
}

/// The byte where unit `k` starts: `Ok` at a character's start (past the end, the end), `Err` with
/// the character's start when `k` falls between the halves of its surrogate pair.
fn offset(s: &str, k: usize) -> Result<usize, usize> {
    let mut u = 0;
    for (b, c) in s.char_indices() {
        if u == k {
            return Ok(b);
        }
        u += c.len_utf16();
        if u > k {
            return Err(b);
        }
    }
    Ok(s.len())
}

/// Units `from..to`: empty when `to <= from`; nothing when an end cuts a surrogate pair.
fn cut(s: &str, from: usize, to: usize) -> Option<&str> {
    if from >= to {
        return Some("");
    }
    match (offset(s, from), offset(s, to)) {
        (Ok(a), Ok(b)) => Some(&s[a..b]),
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

/// A start clamped below only: past the end is the end, which `offset` finds.
fn start(x: f64) -> usize {
    let x = x.trunc();
    if x <= 0.0 { 0 } else { x as usize }
}

/// The first match of `t` at or after unit `k`, as a unit index. A well-formed `t` can't begin on
/// a pair's second half, so a `k` there searches from the next character.
fn find(s: &str, t: &str, k: usize) -> Option<usize> {
    if t.is_empty() {
        return Some(k);
    }
    let from = match offset(s, k) {
        Ok(b) => b,
        Err(b) => b + s[b..].chars().next().map_or(0, char::len_utf8),
    };
    s[from..].find(t).map(|i| units(&s[..from + i]))
}

/// The last match of `t` beginning at or before unit `k`, as a unit index.
fn rfind(s: &str, t: &str, k: usize) -> Option<usize> {
    let (Ok(b) | Err(b)) = offset(s, k);
    let mut limit = (b + t.len()).min(s.len());
    while !s.is_char_boundary(limit) {
        limit -= 1;
    }
    s[..limit].rfind(t).map(|i| units(&s[..i]))
}

fn count(k: usize) -> Value {
    Value::Number(k as f64)
}

fn found(k: Option<usize>) -> Value {
    Value::Number(k.map_or(-1.0, |k| k as f64))
}

// ── the leaves ───────────────────────────────────────────────────────────────────────────────

/// The one unit at `k` as a text: empty past the end, nothing for half of a surrogate pair.
fn unit_at(s: &str, k: usize) -> Option<&str> {
    match offset(s, k) {
        Ok(b) if b == s.len() => Some(""),
        Ok(b) => {
            let c = s[b..].chars().next()?;
            (c.len_utf16() == 1).then(|| &s[b..b + c.len_utf8()])
        }
        Err(_) => None,
    }
}

/// `s.at(i)`: from the end when negative; past either end, nothing (JavaScript's `undefined`).
pub(crate) fn at(s: &str, i: f64) -> Option<Value> {
    let i = i.trunc();
    let k = if i < 0.0 { units(s) as f64 + i } else { i };
    if k < 0.0 {
        return None;
    }
    unit_at(s, k as usize).filter(|u| !u.is_empty()).map(|u| Value::Text(u.to_string()))
}

/// `s.charAt(i)`: past either end, the empty text.
pub(crate) fn char_at(s: &str, i: f64) -> Option<Value> {
    let i = i.trunc();
    if i < 0.0 {
        return Some(Value::Text(String::new()));
    }
    unit_at(s, i as usize).map(|u| Value::Text(u.to_string()))
}

/// `s.charCodeAt(i)` (`"unit"`): the code unit, 0 to 65535. `s.codePointAt(i)` (`"point"`): the
/// code point a pair's first half begins, a second half's own unit. Past either end, nothing
/// (JavaScript's `NaN` and `undefined`).
pub(crate) fn code_at(mode: &str, s: &str, i: f64) -> Option<Value> {
    let i = i.trunc();
    if i < 0.0 {
        return None;
    }
    let k = i as usize;
    match mode {
        "unit" => s.encode_utf16().nth(k).map(|u| Value::Number(f64::from(u))),
        "point" => {
            let mut u = 0;
            for c in s.chars() {
                if u == k {
                    return Some(Value::Number(f64::from(c as u32)));
                }
                if c.len_utf16() == 2 && u + 1 == k {
                    return Some(Value::Number(f64::from(0xDC00 + ((c as u32 - 0x10000) & 0x3FF))));
                }
                u += c.len_utf16();
            }
            None
        }
        _ => None,
    }
}

/// `String.fromCharCode(…)` (`"unit"`: each number ToUint16, truncated modulo 2^16) and
/// `String.fromCodePoint(…)` (`"point"`: each a whole code point, 0 to U+10FFFF), the units
/// joined; none is `""`. Nothing when a lone surrogate is left over, or (`"point"`) a number
/// isn't a code point (JavaScript's RangeError).
pub(crate) fn from_codes(mode: &str, codes: &[&Value]) -> Option<Value> {
    let mut out = Vec::with_capacity(codes.len());
    for x in codes {
        let Value::Number(x) = x else { return None };
        match mode {
            "unit" => out.push(x.trunc().rem_euclid(65536.0) as u16),
            "point" => {
                if x.trunc() != *x || !(0.0..=1_114_111.0).contains(x) {
                    return None;
                }
                let mut pair = [0u16; 2];
                // A surrogate's own number has no `char`: it is pushed as the unit it is, and
                // pairs with its other half, if one follows, when the units are read.
                match char::from_u32(*x as u32) {
                    Some(c) => out.extend_from_slice(c.encode_utf16(&mut pair)),
                    None => out.push(*x as u16),
                }
            }
            _ => return None,
        }
    }
    String::from_utf16(&out).ok().map(Value::Text)
}

/// `s.indexOf(t, from)`: the first `t` at or after `from` (clamped to 0 and the length; 0 when
/// unwired), as a unit index; `""` is found at `from`; -1 when absent.
pub(crate) fn index_of(s: &str, t: &str, from: Option<f64>) -> Value {
    let k = from.map_or(0, |p| clamp(p, units(s)));
    found(find(s, t, k))
}

/// `s.lastIndexOf(t, from)`: the last `t` beginning at or before `from` (clamped to 0 and the
/// length less `t`'s; the end when unwired); -1 when absent or longer than the text.
pub(crate) fn last_index_of(s: &str, t: &str, from: Option<f64>) -> Value {
    let (n, m) = (units(s), units(t));
    if m > n {
        return Value::Number(-1.0);
    }
    let k = from.map_or(n - m, |p| clamp(p, n - m));
    if t.is_empty() {
        return count(k);
    }
    found(rfind(s, t, k))
}

/// `s.includes(t, from)`: whether `t` is found at or after `from` (0 when unwired).
pub(crate) fn includes(s: &str, t: &str, from: Option<f64>) -> Value {
    Value::Boolean(find(s, t, from.map_or(0, start)).is_some())
}

/// `s.startsWith(t, p)` (`"start"`: `t` begins at `p`, 0 when unwired) and `s.endsWith(t, p)`
/// (`"end"`: `t` ends at `p`, the length when unwired). A well-formed `t` can't begin on a pair's
/// second half, nor end on its first, so only `""` begins or ends inside a pair.
pub(crate) fn contains(at: &str, s: &str, t: &str, position: Option<f64>) -> Option<Value> {
    let within = match at {
        "start" => match offset(s, position.map_or(0, start)) {
            Ok(b) => s[b..].starts_with(t),
            Err(_) => t.is_empty(),
        },
        "end" => match position.map_or(Ok(s.len()), |p| offset(s, start(p))) {
            Ok(b) => s[..b].ends_with(t),
            Err(_) => t.is_empty(),
        },
        _ => return None,
    };
    Some(Value::Boolean(within))
}

/// A whole number not below 0: `slice {mode: "length"}`'s start and count.
fn whole(v: &Value) -> Option<usize> {
    match v {
        Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as usize),
        _ => None,
    }
}

/// Part of a text by `mode`. `"length"` (`s.substr(a, n)`): `end` is how many units from `start`,
/// both whole and not negative, cut short at the end. `"end"` (`s.slice(a, b)`): a negative
/// position counts from the end, both clamped to the text, empty when `end` is at or before
/// `start`; `end` unwired is the length. `"substring"` (`s.substring(a, b)`): both clamped to 0
/// and the length, swapped when `start` is past `end`. Nothing when a piece would cut a pair.
pub(crate) fn slice(mode: &str, s: &str, a: &Value, b: Option<&Value>) -> Option<Value> {
    let n = units(s);
    let number = |v: &Value| if let Value::Number(x) = v { Some(*x) } else { None };
    let (from, to) = match mode {
        "length" => {
            let from = whole(a)?.min(n);
            (from, from + whole(b?)?.min(n - from))
        }
        "end" => {
            let from = relative(number(a)?, n);
            (from, b.map_or(Some(n), |b| number(b).map(|b| relative(b, n)))?)
        }
        "substring" => {
            let x = clamp(number(a)?, n);
            let y = b.map_or(Some(n), |b| number(b).map(|b| clamp(b, n)))?;
            (x.min(y), x.max(y))
        }
        _ => return None,
    };
    cut(s, from, to).map(|piece| Value::Text(piece.to_string()))
}

/// `s.split(sep, limit)`: the pieces between each `sep`, `""` cutting every code unit (so
/// `"".split("")` is `[]`). `limit` is ToUint32's (truncated, modulo 2^32) and keeps that many
/// pieces; unwired, all. Nothing for `""` on a text with an astral character, whose pair's halves
/// can't stand alone.
pub(crate) fn split(s: &str, sep: &str, limit: Option<f64>) -> Option<Value> {
    let limit = limit.map_or(usize::MAX, |l| l.trunc().rem_euclid(4294967296.0) as usize);
    let piece = |p: &str| Value::Text(p.to_string());
    if !sep.is_empty() {
        return Some(Value::List(s.split(sep).take(limit).map(piece).collect()));
    }
    let mut out = Vec::new();
    for (b, c) in s.char_indices() {
        if out.len() >= limit {
            break;
        }
        if c.len_utf16() == 2 {
            return None;
        }
        out.push(piece(&s[b..b + c.len_utf8()]));
    }
    Some(Value::List(out))
}

/// JavaScript's WhiteSpace and LineTerminator: not Rust's `char::is_whitespace`, which takes
/// U+0085 and leaves out U+FEFF.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

/// `s.trim()` (`"both"`), `s.trimStart()` (`"start"`) and `s.trimEnd()` (`"end"`): JavaScript's
/// white space and line ends dropped from that side.
pub(crate) fn trim<'s>(side: &str, s: &'s str) -> Option<&'s str> {
    Some(match side {
        "both" => s.trim_matches(js_space),
        "start" => s.trim_start_matches(js_space),
        "end" => s.trim_end_matches(js_space),
        _ => return None,
    })
}

/// `s.padStart(length, fill)` (`"start"`) and `s.padEnd(length, fill)` (`"end"`): unchanged when
/// `length` (truncated) is at most the text's or `fill` is `""`; otherwise `fill` repeated and cut
/// to the units missing, before or after. Nothing when that cuts a pair of the fill, or past
/// [`MAX_TEXT`].
pub(crate) fn pad(side: &str, s: &str, length: f64, fill: &str) -> Option<Value> {
    if !matches!(side, "start" | "end") {
        return None;
    }
    let n = units(s);
    let length = length.trunc();
    if length <= n as f64 || fill.is_empty() {
        return Some(Value::Text(s.to_string()));
    }
    if length > MAX_TEXT as f64 {
        return None;
    }
    let k = length as usize - n;
    let fills = fill.repeat(k.div_ceil(units(fill)));
    let filler = cut(&fills, 0, k)?;
    Some(Value::Text(if side == "end" { format!("{s}{filler}") } else { format!("{filler}{s}") }))
}

/// `s.repeat(count)`: the text `count` times (truncated); `""` for 0 or `""`. Nothing for a
/// negative count (JavaScript's RangeError), or past [`MAX_TEXT`].
pub(crate) fn repeat(s: &str, count: f64) -> Option<Value> {
    let count = count.trunc();
    if count < 0.0 {
        return None;
    }
    if s.is_empty() || count == 0.0 {
        return Some(Value::Text(String::new()));
    }
    if count * units(s) as f64 > MAX_TEXT as f64 {
        return None;
    }
    Some(Value::Text(s.repeat(count as usize)))
}

/// A replacement as GetSubstitution reads it for a text pattern: `$$` is `$`, `$&` the match,
/// `` $` `` what precedes it and `$'` what follows it; any other `$` is itself.
enum Part<'r> {
    Literal(&'r str),
    Matched,
    Before,
    After,
}

fn parts(rep: &str) -> Vec<Part<'_>> {
    let mut out = Vec::new();
    let mut rest = rep;
    while let Some(i) = rest.find('$') {
        let part = match rest.as_bytes().get(i + 1) {
            Some(b'$') => Some(Part::Literal("$")),
            Some(b'&') => Some(Part::Matched),
            Some(b'`') => Some(Part::Before),
            Some(b'\'') => Some(Part::After),
            _ => None,
        };
        match part {
            Some(part) => {
                if i > 0 {
                    out.push(Part::Literal(&rest[..i]));
                }
                out.push(part);
                rest = &rest[i + 2..];
            }
            None => {
                out.push(Part::Literal(&rest[..=i]));
                rest = &rest[i + 1..];
            }
        }
    }
    if !rest.is_empty() {
        out.push(Part::Literal(rest));
    }
    out
}

/// `s.replace(a, b)` (`"first"`) and `s.replaceAll(a, b)` (`"all"`) with a text pattern: the first
/// match, or every match left to right without overlapping. `""` matches at 0, or at every unit
/// boundary. Nothing when that leaves a lone surrogate (`"all"` with `""` on astral text and a
/// replacement that isn't `""`), or the result is past [`MAX_TEXT`].
pub(crate) fn replace(count: &str, s: &str, a: &str, b: &str) -> Option<Value> {
    let all = match count {
        "first" => false,
        "all" => true,
        _ => return None,
    };
    // No part writes nothing (`parts` keeps no empty literal, and the match of `""` is left out),
    // but `` $` `` at the first match and `$'` at the last: what is written grows with every part
    // at every match, so the bound on it bounds the work as well, however many parts there are.
    let rep: Vec<Part<'_>> = parts(b).into_iter().filter(|part| !(a.is_empty() && matches!(part, Part::Matched))).collect();
    // Every unit boundary, the halves of a pair among them: the pieces are units, not characters.
    if all && a.is_empty() && s.chars().any(|c| c.len_utf16() == 2) {
        let su: Vec<u16> = s.encode_utf16().collect();
        let n = su.len();
        let mut total = n;
        for p in 0..=n {
            total = total.saturating_add(substituted_units(&rep, p, n, 0));
            if total > MAX_TEXT {
                return None;
            }
        }
        let mut out = Vec::new();
        for p in 0..=su.len() {
            for part in &rep {
                match part {
                    Part::Literal(l) => out.extend(l.encode_utf16()),
                    Part::Matched => {}
                    Part::Before => out.extend_from_slice(&su[..p]),
                    Part::After => out.extend_from_slice(&su[p..]),
                }
            }
            if p < su.len() {
                out.push(su[p]);
            }
        }
        return String::from_utf16(&out).ok().map(Value::Text);
    }
    let matches: Vec<usize> = match (a.is_empty(), all) {
        (true, false) => vec![0],
        (true, true) => s.char_indices().map(|(i, _)| i).chain([s.len()]).collect(),
        (false, true) => s.match_indices(a).map(|(i, _)| i).collect(),
        (false, false) => s.find(a).into_iter().collect(),
    };
    // Measured first: `$'` on every match of a long text would otherwise grow without bound.
    let (n, m) = (units(s), units(a));
    let (mut total, mut last, mut at_units) = (n - matches.len() * m, 0, 0);
    for &at in &matches {
        at_units += units(&s[last..at]);
        total = total.saturating_add(substituted_units(&rep, at_units, n, m));
        if total > MAX_TEXT {
            return None;
        }
        at_units += m;
        last = at + a.len();
    }
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for at in matches {
        out.push_str(&s[last..at]);
        let end = at + a.len();
        for part in &rep {
            out.push_str(match part {
                Part::Literal(l) => l,
                Part::Matched => &s[at..end],
                Part::Before => &s[..at],
                Part::After => &s[end..],
            });
        }
        last = end;
    }
    out.push_str(&s[last..]);
    Some(Value::Text(out))
}

/// How many units a replacement writes for a match `m` units long at unit `at` of a text `n` long.
fn substituted_units(rep: &[Part<'_>], at: usize, n: usize, m: usize) -> usize {
    rep.iter()
        .map(|part| match part {
            Part::Literal(l) => units(l),
            Part::Matched => m,
            Part::Before => at,
            Part::After => n - at - m,
        })
        .fold(0usize, usize::saturating_add)
}

/// `s.normalize(form)`: Unicode normalization form `"NFC"`, `"NFD"`, `"NFKC"` or `"NFKD"`.
pub(crate) fn normalize(form: &str, s: &str) -> Option<Value> {
    Some(Value::Text(match form {
        "NFC" => s.nfc().collect(),
        "NFD" => s.nfd().collect(),
        "NFKC" => s.nfkc().collect(),
        "NFKD" => s.nfkd().collect(),
        _ => return None,
    }))
}

/// `a < b` and the rest on texts: code unit by code unit, a prefix first, as JavaScript compares.
/// Rust's `str` order is code points', which differs between U+E000–U+FFFF and astral characters.
pub(crate) fn order(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// The characters `encodeURI` keeps beyond `encodeURIComponent`'s, and `decodeURI` leaves escaped.
const RESERVED: &str = ";/?:@&=+$,#";

/// `encodeURIComponent(s)` (`"component"`) and `encodeURI(s)` (`"uri"`): A–Z, a–z, 0–9 and
/// `-_.!~*'()` kept, and for `"uri"` `;/?:@&=+$,#` too; every other character as its UTF-8 bytes,
/// `%XX` in capitals.
pub(crate) fn uri_encode(set: &str, s: &str) -> Option<Value> {
    let keeps_reserved = match set {
        "component" => false,
        "uri" => true,
        _ => return None,
    };
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) || (keeps_reserved && RESERVED.contains(c)) {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    Some(Value::Text(out))
}

/// `decodeURIComponent(s)` (`"component"`) and `decodeURI(s)` (`"uri"`): each `%XX` run read as
/// UTF-8, hex digits in either case; `"uri"` leaves an escape of `;/?:@&=+$,#` as written. Nothing
/// where JavaScript throws a URIError: a malformed escape, invalid UTF-8, an overlong form, a
/// surrogate or a code point past U+10FFFF.
pub(crate) fn uri_decode(set: &str, s: &str) -> Option<Value> {
    let keeps_reserved = match set {
        "component" => false,
        "uri" => true,
        _ => return None,
    };
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
            if keeps_reserved && RESERVED.contains(c) {
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
    Some(Value::Text(out))
}

/// `btoa(s)`: each character one byte (Latin-1, not UTF-8), padded; nothing for a character past
/// U+00FF (JavaScript's InvalidCharacterError).
pub(crate) fn latin1_base64(s: &str) -> Option<String> {
    let bytes = s.chars().map(|c| u8::try_from(c as u32).ok()).collect::<Option<Vec<u8>>>()?;
    Some(crate::crypto::base64_encode(&bytes, crate::crypto::alphabet("standard")?, true))
}

/// `atob(s)`: WHATWG's forgiving-base64 decode (ASCII white space dropped, one or two `=` dropped
/// when the length is a multiple of 4, extra bits dropped), each byte one character (Latin-1).
/// Nothing for a length of 1 modulo 4 or a character outside the alphabet.
pub(crate) fn base64_latin1(s: &str) -> Option<String> {
    let alphabet = crate::crypto::alphabet("standard")?;
    let mut data: Vec<u8> = s.bytes().filter(|b| !matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')).collect();
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
        acc = acc << 6 | alphabet.iter().position(|&a| a == b)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(char::from((acc >> bits) as u8));
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// A value as `+` writes it beside a text: a text itself, a number as JavaScript writes it, a
/// boolean `true` or `false`; nothing for a list or a record (JavaScript's `"1,2"` and
/// `"[object Object]"`: texts.md, "Differences that stay").
fn plus_text(v: &Value) -> Option<Cow<'_, str>> {
    match v {
        Value::Text(s) => Some(Cow::Borrowed(s)),
        Value::Number(x) => Some(Cow::Owned(crate::primitives::number_text(*x))),
        Value::Boolean(b) => Some(Cow::Borrowed(if *b { "true" } else { "false" })),
        _ => None,
    }
}

/// `a + b + …` with something other than numbers among them: a left fold from the first operand.
/// Two numbers add; with a text on either side, both are written as [`plus_text`] writes them and
/// joined, so `1 + 2 + "a"` is `"3a"` and `"a" + 1 + 2` is `"a12"`. Nothing for a boolean without
/// a text (JavaScript's `true + 1` is 2), a list or a record, or a sum past the largest number.
pub(crate) fn plus(operands: &[&Value]) -> Option<Value> {
    let (first, rest) = operands.split_first()?;
    let mut acc = (*first).clone();
    for x in rest {
        acc = match (acc, x) {
            (Value::Number(a), Value::Number(b)) if (a + b).is_finite() => Value::Number(a + b),
            (Value::Text(mut s), x) => {
                s.push_str(&plus_text(x)?);
                Value::Text(s)
            }
            (a, Value::Text(t)) => Value::Text(format!("{}{t}", plus_text(&a)?)),
            _ => return None,
        };
    }
    matches!(acc, Value::Number(_) | Value::Text(_)).then_some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::compute;
    use crate::Param;

    fn p(name: &str, value: &str) -> Param {
        Param::frozen(name, value)
    }

    fn json(s: &str) -> Value {
        crate::json::parse(s).unwrap_or_else(|| panic!("not JSON: {s}"))
    }

    /// The cases Node 22.21.1 computed for the races (`races_texts_oracle.json`, next to the
    /// bench): each method's inputs and JavaScript's answer, `null` where it has no Holon value.
    const ORACLE: &str = include_str!("../../holon-author/benches/races_texts_oracle.json");

    /// The leaf and params a JavaScript method of the oracle reads as, and how its inputs reach
    /// the leaf: as they are, or (`compare`) with the operator moved to the `op` param.
    fn leaf_of(method: &str) -> (&'static str, Vec<Param>) {
        let one = |leaf, k, v| (leaf, vec![p(k, v)]);
        match method {
            "at" => ("at", vec![]),
            "charAt" => ("text_at", vec![]),
            "charCodeAt" => one("code_at", "mode", "unit"),
            "codePointAt" => one("code_at", "mode", "point"),
            "length" => ("text_length", vec![]),
            "slice" => one("slice", "mode", "end"),
            "substring" => one("slice", "mode", "substring"),
            "indexOf" => ("index_of", vec![]),
            "lastIndexOf" => ("last_index_of", vec![]),
            "includes" => ("includes", vec![]),
            "startsWith" => one("text_contains", "at", "start"),
            "endsWith" => one("text_contains", "at", "end"),
            "trim" => one("trim", "side", "both"),
            "trimStart" => one("trim", "side", "start"),
            "trimEnd" => one("trim", "side", "end"),
            "padStart" => one("pad", "side", "start"),
            "padEnd" => one("pad", "side", "end"),
            "repeat" => ("repeat", vec![]),
            "replace" => one("replace", "count", "first"),
            "replaceAll" => one("replace", "count", "all"),
            "split" => ("split", vec![]),
            "fromCharCode" => one("from_codes", "mode", "unit"),
            "fromCodePoint" => one("from_codes", "mode", "point"),
            "compare" => ("compare", vec![]),
            "encodeURIComponent" => one("uri_encode", "set", "component"),
            "encodeURI" => one("uri_encode", "set", "uri"),
            "decodeURIComponent" => one("uri_decode", "set", "component"),
            "decodeURI" => one("uri_decode", "set", "uri"),
            "btoa" => ("base64", vec![p("mode", "encode"), p("alphabet", "standard"), p("bytes", "latin1")]),
            "atob" => ("base64", vec![p("mode", "decode"), p("alphabet", "standard"), p("bytes", "latin1")]),
            "String" => ("as_text", vec![]),
            "+" => ("add", vec![]),
            other => panic!("no leaf for {other}"),
        }
    }

    /// Where a leaf gives nothing by design and JavaScript gives a value (texts.md, "Differences
    /// that stay"): the method and the inputs.
    const STAYS: &[(&str, &str)] = &[("+", "[true, 1]")];

    /// Every text leaf gives JavaScript's answer, in code units, on every case Node computed for
    /// the races, through `compute` as a flow runs it. JavaScript's `null` (no Holon value) reads
    /// as `none()`, where the leaf gives nothing.
    #[test]
    fn the_text_leaves_give_what_javascript_gives() {
        let Some(Value::Record(table)) = crate::json::parse(ORACLE) else { panic!("the oracle's JSON") };
        let null = json("null");
        let mut n = 0;
        for (method, cases) in &table {
            let (leaf, mut params) = leaf_of(method);
            for case in cases.clone().into_list() {
                let [Value::List(inputs), answer] = &case.into_list()[..] else { panic!("{method}: inputs and an answer") };
                let shown = inputs.clone();
                let mut inputs = inputs.clone();
                if method == "compare" {
                    let Some(Value::Text(op)) = inputs.pop() else { panic!("compare: an operator") };
                    params = vec![p("op", &op)];
                }
                let got = compute(leaf, &inputs.iter().collect::<Vec<_>>(), &params);
                let want = (*answer != null).then(|| answer.clone());
                if STAYS.iter().any(|(m, i)| m == method && json(i).into_list() == shown) {
                    assert_eq!(got, None, "{method}{shown:?}: a difference that stays gives nothing");
                } else {
                    assert_eq!(got, want, "{method}{shown:?}: JavaScript gives {want:?}");
                }
                n += 1;
            }
        }
        assert_eq!(n, 1525);
    }

    trait IntoList {
        fn into_list(self) -> Vec<Value>;
    }

    impl IntoList for Value {
        fn into_list(self) -> Vec<Value> {
            match self {
                Value::List(xs) => xs,
                _ => panic!("a list"),
            }
        }
    }

    fn gives(name: &str, params: &[Param], cases: &[(&str, Option<&str>)]) {
        for (inputs, want) in cases {
            let inputs = json(inputs).into_list();
            let want = want.map(json);
            assert_eq!(compute(name, &inputs.iter().collect::<Vec<_>>(), params), want, "{name} {params:?} on {inputs:?}");
        }
    }

    // The races' check cases (crates/holon-author/benches/races_texts.rs), on the edges each spec
    // names.

    #[test]
    fn positions_count_code_units_and_a_cut_pair_gives_nothing() {
        // "a😀b": a, the pair's halves at 1 and 2, b at 3.
        gives("text_length", &[], &[("[\"\"]", Some("0")), ("[\"a😀b\"]", Some("4")), ("[\"e\u{301}x\"]", Some("3")), ("[\"abç\"]", Some("3")), ("[1]", None)]);
        let end = [p("mode", "end")];
        gives("slice", &end, &[("[\"a😀b\", 1, 3]", Some("\"😀\"")), ("[\"a😀b\", 1, 2]", None), ("[\"a😀b\", 2]", None), ("[\"a😀b\", -1]", Some("\"b\"")), ("[\"a😀b\", 3, 1]", Some("\"\""))]);
        let length = [p("mode", "length")];
        gives(
            "slice",
            &length,
            &[
                ("[\"hello\", 1, 3]", Some("\"ell\"")),
                ("[\"hi\", 5, 2]", Some("\"\"")),
                ("[\"a😀b\", 1, 2]", Some("\"😀\"")),
                ("[\"a😀b\", 1, 1]", None),
                ("[\"abc\", -1, 1]", None),
                ("[\"abc\", 0.5, 1]", None),
                ("[\"abc\", 1]", None),
            ],
        );
        let substring = [p("mode", "substring")];
        gives("slice", &substring, &[("[\"abcd\", 3, 1]", Some("\"bc\"")), ("[\"abcd\", -2, 9]", Some("\"abcd\"")), ("[\"abcd\", 2]", Some("\"cd\"")), ("[[1, 2], 0, 1]", None)]);
        gives("index_of", &[], &[("[\"a😀b\", \"b\"]", Some("3")), ("[\"a😀b😀\", \"😀\", 2]", Some("4")), ("[\"abc\", \"\", 9]", Some("3")), ("[\"abc\", 1]", None)]);
        gives("last_index_of", &[], &[("[\"a😀b😀\", \"😀\"]", Some("4")), ("[\"abcabc\", \"bc\", 3]", Some("1")), ("[\"a\", \"abc\"]", Some("-1")), ("[\"abc\", \"\"]", Some("3"))]);
        gives("includes", &[], &[("[\"abc\", \"b\"]", Some("true")), ("[\"abc\", \"a\", 1]", Some("false")), ("[\"abc\", \"c\", -9]", Some("true"))]);
        gives("at", &[], &[("[\"abc\", -1]", Some("\"c\"")), ("[\"abc\", 3]", None), ("[\"a😀b\", 1]", None), ("[\"abc\", 1.9]", Some("\"b\""))]);
        gives("text_at", &[], &[("[\"abc\", -1]", Some("\"\"")), ("[\"abc\", 3]", Some("\"\"")), ("[\"a😀b\", 2]", None), ("[\"abc\", 0]", Some("\"a\""))]);
        gives("code_at", &[p("mode", "unit")], &[("[\"a😀b\", 1]", Some("55357")), ("[\"a😀b\", 2]", Some("56832")), ("[\"a\", 1]", None), ("[\"a\", -1]", None)]);
        gives("code_at", &[p("mode", "point")], &[("[\"a😀b\", 1]", Some("128512")), ("[\"a😀b\", 2]", Some("56832")), ("[\"a😀b\", 3]", Some("98"))]);
    }

    #[test]
    fn from_codes_joins_units_and_a_lone_surrogate_gives_nothing() {
        gives("from_codes", &[p("mode", "unit")], &[("[]", Some("\"\"")), ("[97, 98]", Some("\"ab\"")), ("[55357, 56832]", Some("\"😀\"")), ("[55357]", None), ("[65601]", Some("\"A\"")), ("[-1]", Some("\"\u{FFFF}\""))]);
        gives("from_codes", &[p("mode", "point")], &[("[128512]", Some("\"😀\"")), ("[55357, 56832]", Some("\"😀\"")), ("[1.5]", None), ("[-1]", None), ("[1114112]", None), ("[\"a\"]", None)]);
    }

    #[test]
    fn text_contains_tests_a_start_or_an_end() {
        let start = [p("at", "start")];
        gives("text_contains", &start, &[("[\"abc\", \"ab\"]", Some("true")), ("[\"abc\", \"bc\", 1]", Some("true")), ("[\"a😀b\", \"\", 2]", Some("true")), ("[\"a😀b\", \"b\", 2]", Some("false")), ("[\"abc\", 1]", None)]);
        let end = [p("at", "end")];
        gives("text_contains", &end, &[("[\"abc\", \"bc\"]", Some("true")), ("[\"abc\", \"ab\", 2]", Some("true")), ("[\"abc\", \"c\", -1]", Some("false"))]);
        gives("text_contains", &[p("at", "any")], &[("[\"abc\", \"b\"]", None)]);
    }

    #[test]
    fn split_cuts_code_units_and_keeps_a_limit() {
        gives(
            "split",
            &[],
            &[
                ("[\"a,b,,c,\", \",\"]", Some("[\"a\", \"b\", \"\", \"c\", \"\"]")),
                ("[\"a,b,,c,\", \",\", 2]", Some("[\"a\", \"b\"]")),
                ("[\"a,b\", \",\", -1]", Some("[\"a\", \"b\"]")),
                ("[\"a,b\", \",\", 0]", Some("[]")),
                ("[\"abc\", \"\"]", Some("[\"a\", \"b\", \"c\"]")),
                ("[\"abc\", \"\", 2]", Some("[\"a\", \"b\"]")),
                ("[\"\", \"\"]", Some("[]")),
                ("[\"\", \",\"]", Some("[\"\"]")),
                ("[\"a😀b\", \"\"]", None),
                ("[\"a😀b\", \"\", 1]", Some("[\"a\"]")),
                ("[\"abc\", 1]", None),
            ],
        );
    }

    #[test]
    fn trim_pad_and_repeat_take_javascripts_spaces_and_bounds() {
        let spaces = "\" \\t\\n\\u000b\\f\\r\u{A0}\u{1680}\u{2000}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}x y \"";
        gives("trim", &[p("side", "both")], &[(&format!("[{spaces}]"), Some("\"x y\"")), ("[\"\u{85}x\"]", Some("\"\u{85}x\""))]);
        gives("trim", &[p("side", "start")], &[("[\" a \"]", Some("\"a \""))]);
        gives("trim", &[p("side", "end")], &[("[\" a \"]", Some("\" a\""))]);
        gives("trim", &[p("side", "middle")], &[("[\" a \"]", None)]);
        let start = [p("side", "start")];
        gives(
            "pad",
            &start,
            &[
                ("[\"abc\", 5]", Some("\"  abc\"")),
                ("[\"abc\", 6, \"01\"]", Some("\"010abc\"")),
                ("[\"abc\", 2, \"0\"]", Some("\"abc\"")),
                ("[\"abc\", 9, \"\"]", Some("\"abc\"")),
                ("[\"abc\", 4, \"😀\"]", None),
                ("[\"abc\", 5, \"😀\"]", Some("\"😀abc\"")),
                ("[\"abc\", 1e10, \"0\"]", None),
            ],
        );
        gives("pad", &[p("side", "end")], &[("[\"abc\", 5, \"-\"]", Some("\"abc--\""))]);
        gives("repeat", &[], &[("[\"ab\", 3]", Some("\"ababab\"")), ("[\"ab\", 0]", Some("\"\"")), ("[\"\", 1e300]", Some("\"\"")), ("[\"ab\", -1]", None), ("[\"ab\", 2.9]", Some("\"abab\"")), ("[\"ab\", 1e9]", None)]);
    }

    /// A width or a count from a request makes at most `MAX_TEXT` units: up to it, the text; one
    /// past, nothing, before any of it is made.
    #[test]
    fn a_text_padded_or_repeated_stops_at_the_bound() {
        let a = Value::Text("a".into());
        let at = Value::Number(MAX_TEXT as f64);
        let past = Value::Number(MAX_TEXT as f64 + 1.0);
        let made = |v: Option<Value>| match v {
            Some(Value::Text(s)) => Some(s.len()),
            _ => None,
        };
        assert_eq!(made(compute("repeat", &[&a, &at], &[])), Some(MAX_TEXT));
        assert_eq!(made(compute("repeat", &[&a, &past], &[])), None);
        assert_eq!(made(compute("repeat", &[&Value::Text("ab".into()), &Value::Number((MAX_TEXT / 2 + 1) as f64)], &[])), None);
        assert_eq!(made(compute("pad", &[&a, &at, &a], &[p("side", "start")])), Some(MAX_TEXT));
        assert_eq!(made(compute("pad", &[&a, &past, &a], &[p("side", "end")])), None);
    }

    #[test]
    fn replace_substitutes_as_javascript_does() {
        let first = [p("count", "first")];
        gives("replace", &first, &[("[\"abc\", \"b\", \"[$`]\"]", Some("\"a[a]c\"")), ("[\"abc\", \"b\", \"$$\"]", Some("\"a$c\"")), ("[\"abc\", \"z\", \"-\"]", Some("\"abc\"")), ("[\"abc\", \"\", \"-\"]", Some("\"-abc\""))]);
        let all = [p("count", "all")];
        gives(
            "replace",
            &all,
            &[
                ("[\"abc\", \"b\", \"$&$&\"]", Some("\"abbc\"")),
                ("[\"aaa\", \"aa\", \"x\"]", Some("\"xa\"")),
                ("[\"abc\", \"\", \"-\"]", Some("\"-a-b-c-\"")),
                ("[\"a😀\", \"\", \"\"]", Some("\"a😀\"")),
                ("[\"a😀\", \"\", \"-\"]", None),
                ("[\"ab\", \"b\", \"$'$1$\"]", Some("\"a$1$\"")),
            ],
        );
        gives("replace", &[p("count", "some")], &[("[\"abc\", \"b\", \"x\"]", None)]);
        // `$'` after every unit of a long text: the result would pass MAX_TEXT, so nothing, before
        // any of it is made.
        let long = Value::Text("x".repeat(40_000));
        assert_eq!(compute("replace", &[&long, &Value::Text(String::new()), &Value::Text("$'".into())], &all), None);
        // The match of `""` written at every unit, many times over, writes nothing: the text as
        // it was, at once, where each part was once visited at each match (4 * 10^9 visits here).
        let many = Value::Text("$&".repeat(100_000));
        assert_eq!(compute("replace", &[&long, &Value::Text(String::new()), &many], &all), Some(long.clone()));
        let astral = Value::Text("x😀".repeat(20_000));
        assert_eq!(compute("replace", &[&astral, &Value::Text(String::new()), &many], &all), Some(astral.clone()));
        // A replacement of many parts each writing something, at every unit: past the bound by
        // what it writes, so nothing, the parts visited only until then.
        let parts = Value::Text("$$".repeat(100_000));
        assert_eq!(compute("replace", &[&long, &Value::Text(String::new()), &parts], &all), None);
    }

    #[test]
    fn normalize_composes_and_decomposes() {
        let e = "\"e\u{301}\"";
        gives("normalize", &[p("form", "NFC")], &[(&format!("[{e}]"), Some("\"\u{E9}\""))]);
        gives("normalize", &[p("form", "NFD")], &[("[\"\u{E9}\"]", Some(e))]);
        gives("normalize", &[p("form", "NFKC")], &[("[\"\u{FB01}\"]", Some("\"fi\""))]);
        gives("normalize", &[p("form", "NFKD")], &[("[\"\u{2460}\"]", Some("\"1\""))]);
        gives("normalize", &[p("form", "NFX")], &[("[\"a\"]", None)]);
    }

    #[test]
    fn compare_orders_texts_by_code_units() {
        // U+FF5E is a code point below U+1F600 but a code unit above its first half (U+D83D).
        let lt = [p("op", "<")];
        gives("compare", &lt, &[("[\"\u{FF5E}\", \"😀\"]", Some("false")), ("[\"a\", \"ab\"]", Some("true")), ("[\"B\", \"a\"]", Some("true")), ("[1, 2]", Some("true")), ("[\"1\", 2]", None)]);
        gives("compare", &[p("op", "=")], &[("[\"a\", \"a\"]", Some("true"))]);
        gives("compare", &[p("op", "!=")], &[("[\"a\", \"b\"]", Some("true"))]);
        gives("compare", &[p("op", ">=")], &[("[\"b\", \"a\"]", Some("true"))]);
    }

    #[test]
    fn add_joins_texts_from_the_first_operand() {
        gives(
            "add",
            &[],
            &[
                ("[1, 2, \"a\"]", Some("\"3a\"")),
                ("[\"a\", 1, 2]", Some("\"a12\"")),
                ("[true, \"a\"]", Some("\"truea\"")),
                ("[\"a\", 1e21]", Some("\"a1e+21\"")),
                ("[\"a\"]", Some("\"a\"")),
                ("[true, 1]", None),
                ("[true]", None),
                ("[[1], \"a\"]", None),
                ("[\"a\", {}]", None),
                ("[1e308, 1e308, \"a\"]", None),
            ],
        );
        assert_eq!(compute("add", &[&Value::Number(-0.0), &Value::Text("a".into())], &[]), Some(Value::Text("0a".into())));
    }

    #[test]
    fn uri_encode_and_decode_are_javascripts() {
        gives("uri_encode", &[p("set", "component")], &[("[\"a b&c=d/e?f#g\"]", Some("\"a%20b%26c%3Dd%2Fe%3Ff%23g\"")), ("[\"😀\"]", Some("\"%F0%9F%98%80\""))]);
        gives("uri_encode", &[p("set", "uri")], &[("[\"a b&c=d/e?f#g\"]", Some("\"a%20b&c=d/e?f#g\""))]);
        gives("uri_decode", &[p("set", "component")], &[("[\"%c3%a9%2F\"]", Some("\"\u{E9}/\"")), ("[\"%\"]", None), ("[\"%C0%80\"]", None), ("[\"%ED%A0%80\"]", None)]);
        gives("uri_decode", &[p("set", "uri")], &[("[\"%2F%41\"]", Some("\"%2FA\""))]);
        gives("uri_encode", &[p("set", "path")], &[("[\"a\"]", None)]);
    }

    #[test]
    fn base64_latin1_is_btoa_and_atob() {
        let latin1 = |mode| vec![p("mode", mode), p("alphabet", "standard"), p("bytes", "latin1")];
        gives("base64", &latin1("encode"), &[("[\"\u{FF}\"]", Some("\"/w==\"")), ("[\"😀\"]", None)]);
        gives("base64", &latin1("decode"), &[("[\" /w = = \"]", Some("\"\u{FF}\"")), ("[\"YQ\"]", Some("\"a\"")), ("[\"Y\"]", None), ("[\"Y-==\"]", None)]);
        let url = vec![p("mode", "encode"), p("alphabet", "url"), p("bytes", "latin1")];
        gives("base64", &url, &[("[\"a\"]", None)]);
    }

    #[test]
    fn is_well_formed_is_true_for_a_text() {
        gives("is_well_formed", &[], &[("[\"a😀\"]", Some("true")), ("[\"\"]", Some("true")), ("[1]", None), ("[[\"a\"]]", None)]);
    }
}
