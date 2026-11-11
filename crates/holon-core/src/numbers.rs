//! The number leaves: `math`, `max`, `min`, `aggregate`, `atan2`, `power`, `remainder`, `bitwise`,
//! `number_test`, `to_number` and `number_format`, each JavaScript's meaning on Holon's numbers
//! (docs/engineering/themes/js-library-races/numbers.md, "Leaf specs").
//!
//! A number is a finite double, `-0` included. An input that isn't a number gives nothing where
//! JavaScript would coerce it (`Math.max("5", 3)`), and a result JavaScript gives as `NaN`,
//! `Infinity` or `-Infinity` is nothing. Every result is the same bits on every target: IEEE
//! operations, integer arithmetic, Rust's correctly rounded decimal conversions, and libm (musl's
//! port of fdlibm, pinned in Cargo.lock) for the transcendental functions, never `std`'s, which
//! call the platform's.

use crate::primitives::number_text;
use crate::Value;

/// A finite result, else nothing.
pub(crate) fn number(x: f64) -> Option<Value> {
    x.is_finite().then_some(Value::Number(x))
}

// ── math {fn} ───────────────────────────────────────────────────────────────

/// `Math.<f>(x)`, `f` one of `math`'s functions (catalog `FAMILIES`).
pub(crate) fn math(f: &str, x: f64) -> Option<Value> {
    number(match f {
        "abs" => x.abs(),
        // 1, -1, or x itself for `0` and `-0`.
        "sign" => {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                x
            }
        }
        // IEEE roundToIntegral in its three directions: exact, the sign of a zero kept.
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "trunc" => x.trunc(),
        "round" => js_round(x),
        // Correctly rounded by IEEE; NaN (nothing) below zero, `-0` for `-0`.
        "sqrt" => x.sqrt(),
        "cbrt" => libm::cbrt(x),
        "exp" => libm::exp(x),
        "expm1" => libm::expm1(x),
        "log" => libm::log(x),
        "log2" => libm::log2(x),
        "log10" => libm::log10(x),
        "log1p" => libm::log1p(x),
        "sin" => libm::sin(x),
        "cos" => libm::cos(x),
        "tan" => libm::tan(x),
        "asin" => libm::asin(x),
        "acos" => libm::acos(x),
        "atan" => libm::atan(x),
        "sinh" => libm::sinh(x),
        "cosh" => libm::cosh(x),
        "tanh" => libm::tanh(x),
        "asinh" => libm::asinh(x),
        "acosh" => libm::acosh(x),
        "atanh" => libm::atanh(x),
        // IEEE conversion to binary32, nearest, ties to even: Infinity (nothing) past its range.
        "fround" => (x as f32) as f64,
        "f16round" => f16_round(x),
        "clz32" => to_uint32(x).leading_zeros() as f64,
        _ => return None,
    })
}

/// `Math.round`: the integer closest to x, a tie toward +∞ (`Math.round(-2.5)` is -2), and `-0`
/// for x in [-0.5, -0]. `x - floor(x)` is exact for every double. Not `f64::round`, which rounds a
/// tie away from zero.
fn js_round(x: f64) -> f64 {
    let r = x.floor();
    let y = if x - r >= 0.5 { r + 1.0 } else { r };
    if y == 0.0 { 0.0f64.copysign(x) } else { y }
}

/// `Math.f16round`, by the exponent: x's quantum is 2^(e-10) in binary16's normal range and 2^-24
/// below it; x is divided by it (exact), rounded ties-to-even, and multiplied back (exact).
/// Infinity at 65520 and past it, where binary16 rounds to it.
fn f16_round(x: f64) -> f64 {
    if x == 0.0 {
        return x;
    }
    let a = x.abs();
    if a >= 65520.0 {
        return f64::INFINITY.copysign(x);
    }
    let e = ((a.to_bits() >> 52) & 0x7ff) as i64 - 1023;
    let q = if a >= 6.103515625e-5 { f64::from_bits(((1023 + e - 10) as u64) << 52) } else { 5.960464477539063e-8 };
    ((a / q).round_ties_even() * q).copysign(x)
}

/// ToUint32: truncated, then modulo 2^32 (`rem_euclid` is one exact `%` and one exact addition).
fn to_uint32(x: f64) -> u32 {
    x.trunc().rem_euclid(4294967296.0) as u32
}

/// ToInt32: ToUint32's bits read as signed.
fn to_int32(x: f64) -> i32 {
    to_uint32(x) as i32
}

// ── max, min, aggregate {fn} ────────────────────────────────────────────────

/// `Math.max(a, b)`: `0` over `-0`, whatever the order (`f64::max` may give either).
fn js_max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else if b > a {
        b
    } else if a == 0.0 && (a.is_sign_positive() || b.is_sign_positive()) {
        0.0
    } else {
        a
    }
}

/// `Math.min(a, b)`: `-0` under `0`.
fn js_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else if b < a {
        b
    } else if a == 0.0 && (a.is_sign_negative() || b.is_sign_negative()) {
        -0.0
    } else {
        a
    }
}

/// `max` (`larger`) or `min` of numbers: one gives itself, none gives nothing (JavaScript's
/// `-Infinity` and `Infinity`).
pub(crate) fn extreme<'a>(xs: impl IntoIterator<Item = &'a Value>, larger: bool) -> Option<Value> {
    let mut best: Option<f64> = None;
    for x in xs {
        let Value::Number(x) = x else { return None };
        best = Some(match best {
            None => *x,
            Some(b) if larger => js_max(b, *x),
            Some(b) => js_min(b, *x),
        });
    }
    number(best?)
}

/// One number from a list of numbers, `f` one of `aggregate`'s functions.
pub(crate) fn aggregate(f: &str, items: &[Value]) -> Option<Value> {
    match f {
        "max" => extreme(items, true),
        "min" => extreme(items, false),
        "hypot" => {
            let xs = numbers_of(items)?;
            let (sum, m) = v8_hypot(&xs);
            number(sum.sqrt() * m)
        }
        "sum_precise" => sum_precise(&numbers_of(items)?),
        _ => None,
    }
}

/// A list's items, each a number, else nothing.
fn numbers_of(items: &[Value]) -> Option<Vec<f64>> {
    items.iter().map(|v| if let Value::Number(x) = v { Some(*x) } else { None }).collect()
}

/// V8's `Math.hypot` (builtins/math.tq): the largest magnitude m; each magnitude divided by it,
/// squared and summed in order with Kahan's compensation. `(sum, m)`, the result being
/// `sqrt(sum) * m`; `(0, 0)` when m is 0 or there are none. IEEE operations only, so it is V8's to
/// the bit on every target.
fn v8_hypot(values: &[f64]) -> (f64, f64) {
    let mut m = 0.0;
    for x in values {
        if x.abs() > m {
            m = x.abs();
        }
    }
    if m == 0.0 {
        return (0.0, 0.0);
    }
    let (mut sum, mut comp) = (0.0f64, 0.0f64);
    for x in values {
        let n = x.abs() / m;
        let summand = n * n - comp;
        let p = sum + summand;
        comp = (p - sum) - summand;
        sum = p;
    }
    (sum, m)
}

/// `Math.sumPrecise`: the exact sum, rounded once (nearest, ties to even). Each double is a whole
/// number of 2^-1074, added exactly into a two's-complement accumulator of them wide enough for
/// 2^150 of the largest, so partial sums never overflow. `-0` for no numbers or only `-0`s;
/// nothing when the exact sum rounds past the largest double.
fn sum_precise(xs: &[f64]) -> Option<Value> {
    if xs.iter().all(|x| *x == 0.0 && x.is_sign_negative()) {
        return Some(Value::Number(-0.0));
    }
    const LIMBS: usize = 36;
    let mut acc = [0u64; LIMBS];
    for &x in xs {
        let bits = x.to_bits();
        let (exp, frac) = ((bits >> 52) & 0x7ff, bits & ((1 << 52) - 1));
        // x = ±m × 2^(shift - 1074): a subnormal's fraction counts units, a normal's has its
        // leading 1 at 2^(exp - 1023).
        let (m, shift) = if exp == 0 { (frac, 0) } else { (frac | 1 << 52, exp as usize - 1) };
        let (i, v) = (shift / 64, (m as u128) << (shift % 64));
        let parts = [v as u64, (v >> 64) as u64];
        let mut carry = false;
        for (k, limb) in acc.iter_mut().enumerate().skip(i) {
            let part = parts.get(k - i).copied().unwrap_or(0);
            if k - i >= 2 && !carry {
                break;
            }
            let (r, c1) = if x < 0.0 { limb.overflowing_sub(part) } else { limb.overflowing_add(part) };
            let (r, c2) = if x < 0.0 { r.overflowing_sub(carry as u64) } else { r.overflowing_add(carry as u64) };
            *limb = r;
            carry = c1 || c2;
        }
    }
    let negative = acc[LIMBS - 1] >> 63 == 1;
    if negative {
        // Two's complement: the magnitude is the bits inverted, plus one.
        let mut carry = true;
        for limb in acc.iter_mut() {
            let (r, c) = (!*limb).overflowing_add(carry as u64);
            *limb = r;
            carry = c;
        }
    }
    let total = round_limbs(&acc, -1074);
    number(if negative { -total } else { total })
}

/// `mag × 2^scale` (`mag` in little-endian 64-bit limbs, `scale` at least -1074) rounded once to
/// the nearest double, ties to even: `0` for none, Infinity past the largest.
fn round_limbs(mag: &[u64], scale: i64) -> f64 {
    let Some(top) = mag.iter().rposition(|&l| l != 0) else { return 0.0 };
    let msb = (top * 64 + 63) as i64 - mag[top].leading_zeros() as i64;
    if msb + scale >= 1024 {
        return f64::INFINITY;
    }
    let bit = |i: i64| i >= 0 && (mag[(i / 64) as usize] >> (i % 64)) & 1 == 1;
    // Whether any bit below bit `n` is set.
    let below = |n: i64| {
        let (k, r) = ((n.max(0) / 64) as usize, n.max(0) % 64);
        mag[..k].iter().any(|&l| l != 0) || (r > 0 && mag[k] & ((1u64 << r) - 1) != 0)
    };
    // The lowest bit kept: 53 from the top, but none below 2^-1074 (a subnormal keeps fewer).
    let lowest = (msb - 52).max(-1074 - scale).max(0);
    let mut m = (lowest..=msb).rev().fold(0u64, |m, i| m << 1 | bit(i) as u64);
    if bit(lowest - 1) && (below(lowest - 1) || m & 1 == 1) {
        m += 1;
    }
    // m (at most 2^53, exact as a double) times a power of two the result is a whole number of:
    // exact, or Infinity past the largest.
    let e = lowest + scale;
    let pow2 = if e >= -1022 { f64::from_bits(((e + 1023) as u64) << 52) } else { f64::from_bits(1 << (e + 1074)) };
    m as f64 * pow2
}

// ── bitwise {op} ────────────────────────────────────────────────────────────

/// `a & b`, `a | b`, `a ^ b`, `a << b`, `a >> b`, `a >>> b` or `Math.imul(a, b)`, by `op`: over
/// ToInt32 and ToUint32, integer operations, never `-0`.
pub(crate) fn bitwise(op: &str, a: f64, b: f64) -> Option<Value> {
    let (ia, ib, shift) = (to_int32(a), to_int32(b), to_uint32(b) & 31);
    Some(Value::Number(match op {
        "and" => (ia & ib) as f64,
        "or" => (ia | ib) as f64,
        "xor" => (ia ^ ib) as f64,
        "shl" => ia.wrapping_shl(shift) as f64,
        "shr" => (ia >> shift) as f64,
        "ushr" => (to_uint32(a) >> shift) as f64,
        "imul" => ia.wrapping_mul(ib) as f64,
        _ => return None,
    }))
}

// ── number_test {test, coerce} ──────────────────────────────────────────────

/// Whether `x` is a finite number, an integer, a safe integer or NaN, by `test`. With `coerce`
/// `none` (`Number.isFinite` and the like) a value that isn't a number is false; with `number`
/// (the globals `isFinite`, `isNaN`) it is read by ToNumber first, NaN and ±Infinity kept here.
pub(crate) fn number_test(test: &str, coerce: &str, x: &Value) -> Option<Value> {
    if !matches!(test, "finite" | "integer" | "safe_integer" | "nan") {
        return None;
    }
    let n = match (coerce, x) {
        ("none" | "number", Value::Number(n)) => *n,
        ("none", _) => return Some(Value::Boolean(false)),
        ("number", v) => js_to_number(v),
        _ => return None,
    };
    Some(Value::Boolean(match test {
        "finite" => n.is_finite(),
        "integer" => n.is_finite() && n.trunc() == n,
        "safe_integer" => n.is_finite() && n.trunc() == n && n.abs() <= 9007199254740991.0,
        _ => n.is_nan(),
    }))
}

// ── to_number {grammar} ─────────────────────────────────────────────────────

/// `x` read as a number, by `grammar`: `number` (ToNumber: `Number(x)`, `+x`), `float`
/// (`parseFloat`) or `int` (`parseInt`, in `radix` when given). Nothing when it isn't a finite
/// number.
pub(crate) fn to_number(grammar: &str, x: &Value, radix: Option<&Value>) -> Option<Value> {
    number(match grammar {
        "number" => js_to_number(x),
        "float" => parse_float(&js_string(x)),
        "int" => {
            let radix = match radix {
                None => 0,
                Some(Value::Number(r)) => to_int32(*r),
                Some(_) => return None,
            };
            parse_int(&js_string(x), radix)
        }
        _ => return None,
    })
}

/// JavaScript's StrWhiteSpaceChar: WhiteSpace and LineTerminator (U+FEFF is one, U+0085 isn't,
/// unlike Rust's `char::is_whitespace`).
fn js_space(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// ToString of a Holon value as JavaScript writes it: a number as `number_text`, a list as its
/// items joined with `,`, a record as `[object Object]`.
pub(crate) fn js_string(v: &Value) -> String {
    match v {
        Value::Number(x) => number_text(*x),
        Value::Text(s) => s.clone(),
        Value::Boolean(b) => b.to_string(),
        Value::List(items) => items.iter().map(js_string).collect::<Vec<_>>().join(","),
        // A `Uint8Array`'s, as an array's: its numbers joined with `,`.
        Value::Bytes(b) => b.iter().map(u8::to_string).collect::<Vec<_>>().join(","),
        Value::Record(_) | Value::Sum { .. } | Value::Effect { .. } => "[object Object]".into(),
    }
}

/// ToNumber of a Holon value: `true` is 1, a text by StringToNumber, a list as its `join(",")`
/// (`Number([7])` is 7, `Number([])` 0), a record NaN.
fn js_to_number(v: &Value) -> f64 {
    match v {
        Value::Number(x) => *x,
        Value::Boolean(b) => *b as u8 as f64,
        Value::Text(s) => string_to_number(s),
        Value::List(_) | Value::Bytes(_) => string_to_number(&js_string(v)),
        Value::Record(_) | Value::Sum { .. } | Value::Effect { .. } => f64::NAN,
    }
}

/// StringToNumber: trimmed; empty is 0; `Infinity`; `0x`, `0o`, `0b` integers (no sign); else a
/// decimal literal, correctly rounded (`f64::from_str`); else NaN.
fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(js_space);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            return radix_integer(&t[2..], radix);
        }
    }
    if decimal_prefix(t) == t.len() { t.parse().unwrap_or(f64::NAN) } else { f64::NAN }
}

/// The length of the longest prefix of `t` that is a StrDecimalLiteral without `Infinity`: a
/// sign, digits with an optional point (at least one digit), and an exponent when it has digits.
/// 0 when there is none.
fn decimal_prefix(t: &str) -> usize {
    let b = t.as_bytes();
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - s
    };
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut n = digits(&mut i);
    if i < b.len() && b[i] == b'.' {
        i += 1;
        n += digits(&mut i);
    }
    if n == 0 {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if digits(&mut j) > 0 {
            i = j;
        }
    }
    i
}

/// `parseFloat`: leading white space trimmed, then the longest decimal literal (or `Infinity`)
/// it starts with; NaN for none.
fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(js_space);
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if unsigned.starts_with("Infinity") {
        return if t.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    match decimal_prefix(t) {
        0 => f64::NAN,
        end => t[..end].parse().unwrap_or(f64::NAN),
    }
}

/// `parseInt(s, radix)`, `radix` already ToInt32: leading white space trimmed, a sign, the radix
/// (0 is 10, or 16 after a `0x`; 16 drops a `0x`; outside 2 to 36 NaN), then the longest prefix of
/// its digits, rounded once; NaN for none. A zero keeps the sign (`"-0"` is `-0`).
fn parse_int(s: &str, radix: i32) -> f64 {
    let t = s.trim_start_matches(js_space);
    let negative = t.starts_with('-');
    let mut t = t.strip_prefix(['+', '-']).unwrap_or(t);
    // Only an absent radix (0) or 16 reads a `0x` prefix.
    let (mut radix, prefixed) = match radix {
        0 => (10, true),
        16 => (16, true),
        2..=36 => (radix as u32, false),
        _ => return f64::NAN,
    };
    if prefixed && let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        t = rest;
        radix = 16;
    }
    let end = t.find(|c: char| !c.is_digit(radix)).unwrap_or(t.len());
    let n = radix_integer(&t[..end], radix);
    if negative { -n } else { n }
}

/// An integer's digits in `radix`, rounded once to the nearest double (ties to even): NaN for no
/// digits or one outside the radix. Accumulated exactly; at 2^1088 the value is past the largest
/// double whatever follows, so the rest are only checked.
fn radix_integer(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut limbs: Vec<u64> = Vec::new();
    for c in digits.chars() {
        let Some(d) = c.to_digit(radix) else { return f64::NAN };
        if limbs.len() > 17 {
            continue;
        }
        let mut carry = d as u128;
        for limb in limbs.iter_mut() {
            let v = *limb as u128 * radix as u128 + carry;
            *limb = v as u64;
            carry = v >> 64;
        }
        if carry != 0 {
            limbs.push(carry as u64);
        }
    }
    if limbs.len() > 17 { f64::INFINITY } else { round_limbs(&limbs, 0) }
}

// ── number_format {style} ───────────────────────────────────────────────────

/// `x` written as text, by `style`: `string` (`toString`, in radix `digits` when given), `fixed`
/// (`toFixed`), `precision` (`toPrecision`) or `exponential` (`toExponential`). `digits` is read as
/// JavaScript reads it (truncated); outside its range it gives nothing, where JavaScript throws.
/// Nothing for an `x` that isn't finite either, asked first: the forms below take a number's
/// digits, and it has none.
pub(crate) fn number_format(style: &str, x: f64, digits: Option<&Value>) -> Option<Value> {
    if !x.is_finite() {
        return None;
    }
    let d = match digits {
        None => None,
        Some(Value::Number(d)) => Some(d.trunc()),
        Some(_) => return None,
    };
    let within = |lo: f64, hi: f64| d.filter(|d| (lo..=hi).contains(d)).map(|d| d as usize);
    let text = match (style, d) {
        ("string", None) => number_text(x),
        ("string", Some(_)) => match within(2.0, 36.0)? {
            10 => number_text(x),
            radix => radix_text(x, radix as u32),
        },
        ("fixed", _) => fixed(x, if d.is_none() { 0 } else { within(0.0, 100.0)? }),
        ("precision", None) => number_text(x),
        ("precision", Some(_)) => precision(x, within(1.0, 100.0)?),
        ("exponential", None) => exponential(x, None),
        ("exponential", Some(_)) => exponential(x, Some(within(0.0, 100.0)?)),
        _ => return None,
    };
    Some(Value::Text(text))
}

/// |x|'s exact decimal digits, without trailing zeros, and the exponent of the first:
/// |x| = d₁.d₂d₃… × 10^e. Every double is exact within 767 significant digits, so Rust's exact
/// formatting at 1101 rounds nothing. Zero is `[0]` at 0.
fn exact_digits(x: f64) -> (Vec<u8>, i32) {
    let text = format!("{:.1100e}", x.abs());
    let (mantissa, exp) = text.split_once('e').expect("{:e} writes an exponent");
    let mut digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    (digits, exp.parse().expect("an exponent is a number"))
}

/// The first `k` (at least 1) of `digits` at exponent `e`, rounded half up on the rest: exact,
/// since the digits are, and toward the larger on a tie, as `toFixed`, `toPrecision` and
/// `toExponential` take it. A carry out of the first digit raises the exponent.
fn round_half_up(digits: &[u8], e: i32, k: usize) -> (Vec<u8>, i32) {
    let mut kept: Vec<u8> = digits.iter().copied().take(k).collect();
    kept.resize(k, 0);
    if digits.get(k).is_some_and(|&d| d >= 5) {
        for i in (0..k).rev() {
            if kept[i] < 9 {
                kept[i] += 1;
                return (kept, e);
            }
            kept[i] = 0;
        }
        kept[0] = 1;
        return (kept, e + 1);
    }
    (kept, e)
}

/// Digits as text.
fn text_of(digits: &[u8]) -> String {
    digits.iter().map(|d| (b'0' + d) as char).collect()
}

/// `x.toFixed(f)`: at |x| >= 1e21 as `toString`; else the integer n closest to |x| × 10^f (the
/// larger on a tie) with a point before its last f digits, and `-` for x below zero (`-0` isn't).
fn fixed(x: f64, f: usize) -> String {
    if x.abs() >= 1e21 {
        return number_text(x);
    }
    let (digits, e) = exact_digits(x);
    // The digits n keeps: those down to 10^-f.
    let k = e + 1 + f as i32;
    let n = if k > 0 {
        let (kept, e2) = round_half_up(&digits, e + f as i32, k as usize);
        format!("{}{}", text_of(&kept), "0".repeat((e2 + 1 - k) as usize))
    } else if k == 0 && digits[0] >= 5 {
        "1".to_string()
    } else {
        "0".to_string()
    };
    let n = if n.len() <= f { format!("{}{n}", "0".repeat(f + 1 - n.len())) } else { n };
    let body = if f == 0 { n } else { format!("{}.{}", &n[..n.len() - f], &n[n.len() - f..]) };
    if x < 0.0 { format!("-{body}") } else { body }
}

/// `x.toPrecision(p)`: p significant digits (the larger on a tie), in exponent form when the
/// exponent is below -6 or at least p.
fn precision(x: f64, p: usize) -> String {
    let (n, e) = if x == 0.0 {
        (vec![0; p], 0)
    } else {
        let (digits, e) = exact_digits(x);
        round_half_up(&digits, e, p)
    };
    let n = text_of(&n);
    let body = if e < -6 || e >= p as i32 {
        exponent_form(&n, e)
    } else if e == p as i32 - 1 {
        n
    } else if e >= 0 {
        format!("{}.{}", &n[..e as usize + 1], &n[e as usize + 1..])
    } else {
        format!("0.{}{n}", "0".repeat((-(e + 1)) as usize))
    };
    if x < 0.0 { format!("-{body}") } else { body }
}

/// `x.toExponential(f)`: f digits after the point (the larger on a tie), or as many as the
/// shortest round trip needs when absent.
fn exponential(x: f64, f: Option<usize>) -> String {
    let (n, e) = match f {
        _ if x == 0.0 => ("0".repeat(f.unwrap_or(0) + 1), 0),
        Some(f) => {
            let (digits, e) = exact_digits(x);
            let (n, e) = round_half_up(&digits, e, f + 1);
            (text_of(&n), e)
        }
        None => {
            // Rust's `{:e}` writes the shortest digits that read back as x.
            let text = format!("{:e}", x.abs());
            let (mantissa, exp) = text.split_once('e').expect("{:e} writes an exponent");
            (mantissa.replace('.', ""), exp.parse().expect("an exponent is a number"))
        }
    };
    let body = exponent_form(&n, e);
    if x < 0.0 { format!("-{body}") } else { body }
}

/// Digits at exponent `e` as `d.ddde+e`.
fn exponent_form(n: &str, e: i32) -> String {
    let rest = if n.len() > 1 { format!(".{}", &n[1..]) } else { String::new() };
    format!("{}{rest}e{}{}", &n[..1], if e >= 0 { '+' } else { '-' }, e.abs())
}

/// `x.toString(radix)` for a radix other than 10: V8's `DoubleToRadixCString`. The integer part
/// exactly; fraction digits while they are still within half the gap to the next double, the last
/// rounded (ties to even) with a carry back through those written. IEEE operations only, so the
/// text is V8's on every target.
fn radix_text(x: f64, radix: u32) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let r = radix as f64;
    let value = x.abs();
    let mut integer = value.floor();
    let mut fraction = value - integer;
    // Only digits up to the double's own precision.
    let mut delta = (0.5 * (f64::from_bits(value.to_bits() + 1) - value)).max(f64::from_bits(1));
    let mut fraction_digits: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= r;
            delta *= r;
            let digit = fraction as u8;
            fraction_digits.push(digit);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                // Round up, carrying back through the digits written, into the integer past them.
                loop {
                    match fraction_digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(d) if (d as u32) + 1 < radix => {
                            fraction_digits.push(d + 1);
                            break;
                        }
                        Some(_) => {}
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    // The integer part, last digit first; digits below a double's precision are zeros.
    let exponent = |v: f64| {
        let biased = ((v.to_bits() >> 52) & 0x7ff) as i32;
        if biased == 0 { -1074 } else { biased - 1075 }
    };
    let mut integer_digits: Vec<u8> = Vec::new();
    while exponent(integer / r) > 0 {
        integer /= r;
        integer_digits.push(b'0');
    }
    loop {
        let remainder = libm::fmod(integer, r);
        integer_digits.push(CHARS[remainder as usize]);
        integer = (integer - remainder) / r;
        if integer <= 0.0 {
            break;
        }
    }
    let mut text = String::new();
    if x < 0.0 {
        text.push('-');
    }
    text.extend(integer_digits.iter().rev().map(|&c| c as char));
    if !fraction_digits.is_empty() {
        text.push('.');
        text.extend(fraction_digits.iter().map(|&d| CHARS[d as usize] as char));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(x: f64) -> Value {
        Value::Number(x)
    }
    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    /// A number result's bits, or none.
    fn bits(v: Option<Value>) -> Option<u64> {
        match v {
            Some(Value::Number(x)) => Some(x.to_bits()),
            None => None,
            other => panic!("not a number: {other:?}"),
        }
    }
    fn libm_of(f: &str, x: f64) -> f64 {
        match f {
            "exp" => libm::exp(x),
            "log" => libm::log(x),
            "atan" => libm::atan(x),
            _ => libm::sin(x),
        }
    }
    fn text(v: Option<Value>) -> String {
        match v {
            Some(Value::Text(s)) => s,
            other => panic!("not a text: {other:?}"),
        }
    }

    #[test]
    fn math_rounds_as_javascript_does() {
        let m = |f: &str, x: f64| bits(math(f, x));
        assert_eq!(m("round", -2.5), Some((-2.0f64).to_bits()));
        assert_eq!(m("round", 2.5), Some(3.0f64.to_bits()));
        assert_eq!(m("round", -0.5), Some((-0.0f64).to_bits()));
        assert_eq!(m("round", 0.49999999999999994), Some(0.0f64.to_bits()));
        assert_eq!(m("ceil", -0.5), Some((-0.0f64).to_bits()));
        assert_eq!(m("trunc", -0.5), Some((-0.0f64).to_bits()));
        assert_eq!(m("abs", -0.0), Some(0.0f64.to_bits()));
        assert_eq!(m("sign", -0.0), Some((-0.0f64).to_bits()));
        assert_eq!(m("sign", -7.0), Some((-1.0f64).to_bits()));
        assert_eq!(m("sqrt", -0.0), Some((-0.0f64).to_bits()));
        assert_eq!(m("sqrt", -1.0), None);
        // Outside each domain, or past the largest double: nothing.
        for (f, x) in [("log", 0.0), ("log", -1.0), ("log2", 0.0), ("log10", -0.0), ("log1p", -1.0), ("asin", 1.5), ("acos", -1.5), ("acosh", 0.5), ("atanh", 1.0), ("exp", 710.0), ("expm1", 710.0), ("cosh", 711.0), ("sinh", -711.0)] {
            assert_eq!(m(f, x), None, "{f}({x})");
        }
        // Signed zeros come through.
        for f in ["sin", "tan", "asinh", "expm1", "log1p", "cbrt", "atan", "tanh", "fround", "f16round"] {
            assert_eq!(m(f, -0.0), Some((-0.0f64).to_bits()), "{f}(-0)");
        }
        assert_eq!(m("fround", 5.5), Some(5.5f64.to_bits()));
        assert_eq!(m("fround", 5.05), Some(5.050000190734863f64.to_bits()));
        assert_eq!(m("fround", 3.4028235677973366e38), None);
        assert_eq!(m("f16round", 5.0005), Some(5.0f64.to_bits()));
        assert_eq!(m("f16round", 65519.99), Some(65504.0f64.to_bits()));
        assert_eq!(m("f16round", 65520.0), None);
        assert_eq!(m("clz32", 1.0), Some(31.0f64.to_bits()));
        assert_eq!(m("clz32", 0.0), Some(32.0f64.to_bits()));
        assert_eq!(m("clz32", -1.0), Some(0.0f64.to_bits()));
        assert_eq!(m("clz32", 4294967296.5), Some(32.0f64.to_bits()));
        assert_eq!(m("nope", 1.0), None);
        // The transcendental functions are libm's bits, which may differ from an engine's in the
        // last place (`Math.exp(1)` is `Math.E` in V8; libm's is one ulp above): within one ulp of
        // the true value.
        for (f, x, want) in [("exp", 1.0, std::f64::consts::E), ("log", 10.0, std::f64::consts::LN_10), ("atan", 1.0, std::f64::consts::FRAC_PI_4), ("sin", std::f64::consts::FRAC_PI_6, 0.5)] {
            let got = m(f, x).expect("a number");
            assert!(got.abs_diff(want.to_bits()) <= 1, "{f}({x})");
            assert_eq!(got, libm_of(f, x).to_bits(), "{f} is libm's");
        }
    }

    #[test]
    fn max_and_min_order_signed_zeros() {
        let e = |xs: &[f64], larger: bool| bits(extreme(xs.iter().map(|&x| num(x)).collect::<Vec<_>>().iter(), larger));
        assert_eq!(e(&[-0.0, 0.0], true), Some(0.0f64.to_bits()));
        assert_eq!(e(&[0.0, -0.0], true), Some(0.0f64.to_bits()));
        assert_eq!(e(&[0.0, -0.0], false), Some((-0.0f64).to_bits()));
        assert_eq!(e(&[-0.0, 0.0], false), Some((-0.0f64).to_bits()));
        assert_eq!(e(&[3.0, 9.0, -2.0], true), Some(9.0f64.to_bits()));
        assert_eq!(e(&[3.0, 9.0, -2.0], false), Some((-2.0f64).to_bits()));
        assert_eq!(e(&[4.0], true), Some(4.0f64.to_bits()));
        assert_eq!(e(&[], true), None);
        assert_eq!(extreme(&[num(1.0), txt("5")], true), None);
    }

    #[test]
    fn aggregate_hypot_and_sum_precise() {
        let a = |f: &str, xs: &[f64]| bits(aggregate(f, &xs.iter().map(|&x| num(x)).collect::<Vec<_>>()));
        assert_eq!(a("hypot", &[3.0, 4.0]), Some(5.0f64.to_bits()));
        assert_eq!(a("hypot", &[]), Some(0.0f64.to_bits()));
        assert_eq!(a("hypot", &[-0.0]), Some(0.0f64.to_bits()));
        assert_eq!(a("hypot", &[f64::MAX, f64::MAX]), None);
        assert_eq!(a("max", &[]), None);
        assert_eq!(a("min", &[2.0, -0.0, 0.0]), Some((-0.0f64).to_bits()));
        assert_eq!(a("sum_precise", &[]), Some((-0.0f64).to_bits()));
        assert_eq!(a("sum_precise", &[-0.0, -0.0]), Some((-0.0f64).to_bits()));
        assert_eq!(a("sum_precise", &[-0.0, 0.0]), Some(0.0f64.to_bits()));
        assert_eq!(a("sum_precise", &[1.0, -1.0]), Some(0.0f64.to_bits()));
        assert_eq!(a("sum_precise", &[0.1, 0.2, 0.3]), Some(0.6f64.to_bits()));
        assert_eq!(a("sum_precise", &[1e20, 0.1, -1e20]), Some(0.1f64.to_bits()));
        assert_eq!(a("sum_precise", &[1.0, 1e100, 1.0, -1e100]), Some(2.0f64.to_bits()));
        // Partial sums past the largest double that come back are the exact sum.
        assert_eq!(a("sum_precise", &[f64::MAX, f64::MAX, -f64::MAX]), Some(f64::MAX.to_bits()));
        assert_eq!(a("sum_precise", &[f64::MAX, f64::MAX]), None);
        assert_eq!(a("sum_precise", &[-f64::MAX, -f64::MAX]), None);
        // Subnormals add exactly; a tie rounds to even.
        assert_eq!(a("sum_precise", &[5e-324, 5e-324]), Some(1e-323f64.to_bits()));
        assert_eq!(a("sum_precise", &[-5e-324]), Some((-5e-324f64).to_bits()));
        assert_eq!(a("sum_precise", &[1.0, 1.1102230246251565e-16]), Some(1.0f64.to_bits()));
        assert_eq!(a("sum_precise", &[1.0, 1.1102230246251565e-16, 5e-324]), Some(1.0000000000000002f64.to_bits()));
        assert_eq!(a("sum_precise", &[1.0000000000000002, 1.1102230246251565e-16]), Some(1.0000000000000004f64.to_bits()));
        // Every sum of a few doubles agrees with the naive sum when that one is exact.
        for (x, y) in [(1.5, 2.25), (-3.0, 1e-3), (123456.0, -0.5)] {
            assert_eq!(a("sum_precise", &[x, y]), Some((x + y).to_bits()));
        }
        assert_eq!(aggregate("max", &[num(1.0), txt("2")]), None);
        assert_eq!(aggregate("median", &[num(1.0)]), None);
    }

    #[test]
    fn bitwise_works_on_int32_and_uint32() {
        let b = |op: &str, a: f64, c: f64| bits(bitwise(op, a, c)).map(f64::from_bits);
        assert_eq!(b("and", 12.0, 10.0), Some(8.0));
        assert_eq!(b("or", 12.0, 10.0), Some(14.0));
        assert_eq!(b("xor", 12.0, 10.0), Some(6.0));
        assert_eq!(b("xor", 5.0, -1.0), Some(-6.0));
        assert_eq!(b("shl", 1.0, 31.0), Some(-2147483648.0));
        assert_eq!(b("shl", 1.0, 32.0), Some(1.0));
        assert_eq!(b("shr", -16.0, 2.0), Some(-4.0));
        assert_eq!(b("ushr", -1.0, 0.0), Some(4294967295.0));
        assert_eq!(b("ushr", -16.0, 28.0), Some(15.0));
        assert_eq!(b("imul", 2654435761.0, 2.0), Some(1013904226.0));
        assert_eq!(b("imul", -5.0, 12.0), Some(-60.0));
        assert_eq!(b("and", 4294967296.5, 7.0), Some(0.0));
        assert_eq!(bits(bitwise("and", -0.0, -0.0)), Some(0.0f64.to_bits()), "never -0");
        assert_eq!(b("nand", 1.0, 1.0), None);
    }

    #[test]
    fn number_test_with_and_without_coercion() {
        let t = |test: &str, coerce: &str, x: Value| number_test(test, coerce, &x);
        let yes = Some(Value::Boolean(true));
        let no = Some(Value::Boolean(false));
        assert_eq!(t("finite", "none", num(1.5)), yes);
        assert_eq!(t("finite", "none", txt("5")), no);
        assert_eq!(t("finite", "number", txt("5")), yes);
        assert_eq!(t("finite", "number", txt("1e999")), no);
        assert_eq!(t("integer", "none", num(-0.0)), yes);
        assert_eq!(t("integer", "none", num(1.5)), no);
        assert_eq!(t("safe_integer", "none", num(9007199254740991.0)), yes);
        assert_eq!(t("safe_integer", "none", num(9007199254740992.0)), no);
        assert_eq!(t("nan", "none", txt("abc")), no);
        assert_eq!(t("nan", "number", txt("abc")), yes);
        assert_eq!(t("nan", "number", txt("Infinity")), no);
        assert_eq!(t("nan", "number", num(1.0)), no);
        assert_eq!(t("nan", "number", Value::Record(Default::default())), yes);
        assert_eq!(t("finite", "number", Value::List(vec![num(7.0)])), yes);
        assert_eq!(t("finite", "maybe", num(1.0)), None);
        assert_eq!(t("even", "none", num(1.0)), None);
    }

    #[test]
    fn to_number_reads_as_javascript_does() {
        let n = |g: &str, x: Value| bits(to_number(g, &x, None)).map(f64::from_bits);
        let r = |s: &str, radix: f64| bits(to_number("int", &txt(s), Some(&num(radix)))).map(f64::from_bits);
        // ToNumber.
        assert_eq!(n("number", txt("  42\n")), Some(42.0));
        assert_eq!(n("number", txt("\u{feff}7")), Some(7.0));
        assert_eq!(n("number", txt("\u{85}7")), None);
        assert_eq!(n("number", txt("")), Some(0.0));
        assert_eq!(n("number", txt("Infinity")), None);
        assert_eq!(n("number", txt("0x1F")), Some(31.0));
        assert_eq!(n("number", txt("-0x1F")), None);
        assert_eq!(n("number", txt("0b101")), Some(5.0));
        assert_eq!(n("number", txt("0o17")), Some(15.0));
        assert_eq!(n("number", txt("0x")), None);
        assert_eq!(n("number", txt(".5")), Some(0.5));
        assert_eq!(n("number", txt("5.")), Some(5.0));
        assert_eq!(n("number", txt("1e3")), Some(1000.0));
        assert_eq!(n("number", txt("1_000")), None);
        assert_eq!(n("number", txt("abc")), None);
        assert_eq!(n("number", txt("1e999")), None);
        assert_eq!(bits(to_number("number", &txt("-0"), None)), Some((-0.0f64).to_bits()));
        assert_eq!(n("number", Value::Boolean(true)), Some(1.0));
        assert_eq!(n("number", Value::List(vec![])), Some(0.0));
        assert_eq!(n("number", Value::List(vec![num(7.0)])), Some(7.0));
        assert_eq!(n("number", Value::List(vec![num(1.0), num(2.0)])), None);
        assert_eq!(n("number", Value::Record(Default::default())), None);
        // A hexadecimal integer past 2^53 is rounded once.
        assert_eq!(n("number", txt("0x20000000000001")), Some(9007199254740992.0));
        assert_eq!(n("number", txt("0x20000000000003")), Some(9007199254740996.0));
        assert_eq!(n("number", txt("0x20000000000001000000000000000000000001")), Some(f64::from_bits(0x4940000000000001)));
        // parseFloat.
        assert_eq!(n("float", txt("  3.25abc")), Some(3.25));
        assert_eq!(n("float", txt("1e")), Some(1.0));
        assert_eq!(n("float", txt("1.2.3")), Some(1.2));
        assert_eq!(n("float", txt("0x1F")), Some(0.0));
        assert_eq!(n("float", txt("-Infinity")), None);
        assert_eq!(n("float", txt("abc")), None);
        assert_eq!(n("float", txt(".")), None);
        assert_eq!(bits(to_number("float", &txt("-0"), None)), Some((-0.0f64).to_bits()));
        assert_eq!(n("float", num(1e21)), Some(1e21));
        // parseInt.
        assert_eq!(n("int", txt(" -42px")), Some(-42.0));
        assert_eq!(n("int", txt("0x1F")), Some(31.0));
        assert_eq!(n("int", txt("-0x1F")), Some(-31.0));
        assert_eq!(n("int", txt("3.9")), Some(3.0));
        assert_eq!(n("int", num(0.0000005)), Some(5.0));
        assert_eq!(n("int", txt("")), None);
        assert_eq!(bits(to_number("int", &txt("-0"), None)), Some((-0.0f64).to_bits()));
        assert_eq!(r("ff", 16.0), Some(255.0));
        assert_eq!(r("0xff", 16.0), Some(255.0));
        assert_eq!(r("0xff", 10.0), Some(0.0));
        assert_eq!(r("101", 2.0), Some(5.0));
        assert_eq!(r("z", 36.0), Some(35.0));
        assert_eq!(r("12", 0.0), Some(12.0));
        assert_eq!(r("12", 1.0), None);
        assert_eq!(r("12", 37.0), None);
        assert_eq!(r("12", 16.9), Some(18.0));
        assert_eq!(r("9007199254740993", 10.0), Some(9007199254740992.0));
        assert_eq!(r(&"1".repeat(400), 10.0), None);
        assert_eq!(to_number("int", &txt("1"), Some(&txt("10"))), None);
        assert_eq!(to_number("roman", &txt("1"), None), None);
    }

    #[test]
    fn number_format_writes_as_javascript_does() {
        let f = |style: &str, x: f64, d: Option<f64>| text(number_format(style, x, d.map(num).as_ref()));
        // toFixed: the larger on an exact tie, from the exact binary value.
        assert_eq!(f("fixed", 1.005, Some(2.0)), "1.00");
        assert_eq!(f("fixed", 2.5, Some(0.0)), "3");
        assert_eq!(f("fixed", -2.5, Some(0.0)), "-3");
        assert_eq!(f("fixed", -0.0, Some(2.0)), "0.00");
        assert_eq!(f("fixed", -0.0001, Some(2.0)), "-0.00");
        assert_eq!(f("fixed", 0.5, None), "1");
        assert_eq!(f("fixed", 9.995, Some(2.0)), "9.99");
        assert_eq!(f("fixed", 9.5, Some(0.0)), "10");
        assert_eq!(f("fixed", 99.995, Some(1.0)), "100.0");
        assert_eq!(f("fixed", 0.000001, Some(2.0)), "0.00");
        assert_eq!(f("fixed", 0.005, Some(2.0)), "0.01");
        assert_eq!(f("fixed", 1234.5678, Some(2.0)), "1234.57");
        assert_eq!(f("fixed", 1e21, Some(2.0)), "1e+21");
        assert_eq!(f("fixed", 123.456, Some(10.0)), "123.4560000000");
        assert_eq!(f("fixed", 0.1, Some(20.0)), "0.10000000000000000555");
        // toPrecision.
        assert_eq!(f("precision", 123.456, Some(4.0)), "123.5");
        assert_eq!(f("precision", 0.00001234, Some(2.0)), "0.000012");
        assert_eq!(f("precision", 0.0000001234, Some(2.0)), "1.2e-7");
        assert_eq!(f("precision", 123456.0, Some(2.0)), "1.2e+5");
        assert_eq!(f("precision", 99.99, Some(3.0)), "100");
        assert_eq!(f("precision", 0.0, Some(3.0)), "0.00");
        assert_eq!(f("precision", -1.5, Some(1.0)), "-2");
        assert_eq!(f("precision", 1.5, None), "1.5");
        // toExponential.
        assert_eq!(f("exponential", 123456.0, Some(2.0)), "1.23e+5");
        assert_eq!(f("exponential", 123456.0, None), "1.23456e+5");
        assert_eq!(f("exponential", 0.00015, Some(1.0)), "1.5e-4");
        assert_eq!(f("exponential", 0.0, None), "0e+0");
        assert_eq!(f("exponential", 0.0, Some(2.0)), "0.00e+0");
        assert_eq!(f("exponential", -5.0, Some(0.0)), "-5e+0");
        assert_eq!(f("exponential", 9.96, Some(1.0)), "1.0e+1");
        // toString, in a radix.
        assert_eq!(f("string", 255.0, Some(16.0)), "ff");
        assert_eq!(f("string", -255.0, Some(2.0)), "-11111111");
        assert_eq!(f("string", 0.5, Some(2.0)), "0.1");
        assert_eq!(f("string", 0.1, Some(2.0)), "0.0001100110011001100110011001100110011001100110011001101");
        assert_eq!(f("string", 0.1, Some(16.0)), "0.1999999999999a");
        assert_eq!(f("string", std::f64::consts::PI, Some(16.0)), "3.243f6a8885a3");
        assert_eq!(f("string", 1e21, Some(16.0)), "3635c9adc5dea00000");
        assert_eq!(f("string", -0.0, Some(2.0)), "0");
        assert_eq!(f("string", 35.0, Some(36.0)), "z");
        assert_eq!(f("string", 1.5, Some(10.0)), "1.5");
        assert_eq!(f("string", 1e21, None), "1e+21");
        assert_eq!(f("string", 0.0000001, None), "1e-7");
        // Digits outside their range, or not a number: nothing.
        let none = |style: &str, d: Value| number_format(style, 1.0, Some(&d));
        assert_eq!(none("fixed", num(101.0)), None);
        assert_eq!(none("fixed", num(-1.0)), None);
        assert_eq!(none("precision", num(0.0)), None);
        assert_eq!(none("exponential", num(101.0)), None);
        assert_eq!(none("string", num(1.0)), None);
        assert_eq!(none("string", num(37.0)), None);
        assert_eq!(none("fixed", txt("2")), None);
        assert_eq!(number_format("money", 1.0, None), None);
        // A digits value is truncated, as JavaScript reads it.
        assert_eq!(f("fixed", 1.25, Some(1.9)), "1.3");
        // A number that isn't finite (a module's answer) has no digits to write: nothing, in
        // every style, asked before any form is tried. In radix 2 it once asked for 64 GiB.
        for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for (style, d) in [("string", None), ("string", Some(2.0)), ("string", Some(10.0)), ("fixed", Some(2.0)), ("precision", Some(3.0)), ("exponential", None)] {
                assert_eq!(number_format(style, x, d.map(num).as_ref()), None, "{style} of {x}");
            }
        }
    }
}
