//! Numbers: each gap of JavaScript's `Math`, `Number` and numeric operators, a composite against a
//! native candidate (docs/engineering/themes/javascript-library.md §11, "Numbers";
//! docs/engineering/themes/js-library-races/numbers.md has the verdicts).
//!
//! Each native candidate is a Rust body with exactly JavaScript's meaning on Holon's values (finite
//! doubles, `-0` included): nothing where JavaScript gives `NaN` or `±Infinity`. Each composite is
//! checked equal to its candidate by bits on edge cases and a few thousand fixed pseudo-random
//! inputs, then raced as one call and mapped over 1, 64 and 4096 items. A few races put a one-leaf
//! stand-in where a leaf this page proposes would go ("projection"): times only, no check.

mod race;

use std::hint::black_box;

use holon_core::Value;
use race::{check, function, mapped, num, output, race, same, text, v, Native, Side};

/// The sizes a scalar composite is raced at when mapped.
const SIZES: &[usize] = &[1, 64, 4096];

/// 2^52: at and above it every double is an integer, and adding it to a smaller magnitude rounds
/// that magnitude to an integer (IEEE round to nearest, ties to even).
const C52: f64 = 4503599627370496.0;

// ── values ───────────────────────────────────────────────────────────────────────────────────────

/// A list of `n` numbers of both signs with fractions, so a rounding composite takes each path.
fn mixed(n: usize) -> Value {
    Value::List((0..n).map(|k| Value::Number((k as f64 - n as f64 / 2.0) * 0.75 + 0.25)).collect())
}

fn number(x: f64) -> Option<Value> {
    x.is_finite().then_some(Value::Number(x))
}

fn boolean(b: bool) -> Option<Value> {
    Some(Value::Boolean(b))
}

fn num1(xs: &[&Value]) -> Option<f64> {
    if let [Value::Number(x)] = xs { Some(*x) } else { None }
}

fn num2(xs: &[&Value]) -> Option<(f64, f64)> {
    if let [Value::Number(a), Value::Number(b)] = xs { Some((*a, *b)) } else { None }
}

/// A fixed xorshift, so every run checks the same inputs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The edge cases every unary check runs: signed zeros, halves, the largest double under 0.5,
/// 2^52 and its neighbours, integers above 2^53, the smallest and largest doubles, the int32 and
/// uint32 boundaries, and float32 and float16 boundaries and ties.
fn edges() -> Vec<f64> {
    vec![
        0.0, -0.0, 0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 3.5, -3.5, 0.49999999999999994, -0.49999999999999994,
        0.5000000000000001, -0.5000000000000001, 1.0, -1.0, 0.1, -0.1, 0.7, -0.7, 123.456, -123.456,
        C52 - 0.5, -(C52 - 0.5), C52, -C52, C52 + 1.0, -C52 - 1.0, 2.0 * C52 + 2.0, -(2.0 * C52 + 2.0),
        9007199254740991.0, 9007199254740992.0, -9007199254740992.0, 18014398509481988.0, 1e21, -1e21,
        1e300, -1e300, f64::MAX, f64::MIN, 5e-324, -5e-324, 1e-300, -1e-300,
        2147483648.0, -2147483648.0, 2147483647.5, -2147483648.5, 4294967296.0, 4294967295.0, 4294967296.5,
        -4294967296.5, 4294967301.0, -4294967301.0, 65504.0, 65519.99, 65520.0, -65520.0, 65519.0,
        6.103515625e-05, 6.1e-05, -6.1e-05, 5.960464477539063e-8, 2.9802322387695312e-8, -2.9802322387695312e-8,
        3.4028234663852886e38, 3.4028235677973366e38, 3.402823567797336e38, -3.402823567797336e38,
        1.1754943508222875e-38, 1.17549435e-38, 1.401298464324817e-45, 7.006492321624085e-46, -7.006492321624085e-46,
        1.0000000596046448, 1.0000001788139343, 1.00048828125, 1.00146484375, 2049.0, 2051.0,
    ]
}

/// Pseudo-random doubles: any exponent, both signs; fractions within ±8192; integers within ±2^33;
/// halves; and values whose mantissa is a float32 or float16 tie.
fn randoms(rng: &mut Rng, n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let r = rng.next();
        let x = match out.len() % 6 {
            0 => f64::from_bits(r),
            1 => (r % (1 << 20)) as f64 / 64.0 - 8192.0,
            2 => ((r % (1u64 << 34)) as i64 - (1i64 << 33)) as f64,
            3 => ((r % 20000) as f64 - 10000.0) + 0.5,
            4 => f64::from_bits((r & 0x800F_FFFF_E000_0000 | (1 << 28)) | ((1023 - 100 + (rng.next() % 200)) << 52)),
            _ => f64::from_bits((r & 0x800F_FC00_0000_0000 | (1 << 41)) | ((1023 - 14 + (rng.next() % 30)) << 52)),
        };
        if x.is_finite() {
            out.push(x);
        }
    }
    out
}

fn unary_cases(rng: &mut Rng) -> Vec<Vec<Value>> {
    edges().into_iter().chain(randoms(rng, 3000)).map(|x| vec![num(x)]).collect()
}

/// Every pair of `set`, then `n` random pairs of moderate magnitude.
fn pairs(set: &[f64], rng: &mut Rng, n: usize) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = set.iter().flat_map(|&a| set.iter().map(move |&b| vec![num(a), num(b)])).collect();
    let xs = randoms(rng, 2 * n);
    out.extend(xs.chunks(2).map(|p| vec![num(p[0]), num(p[1])]));
    out
}

// ── native candidates: JavaScript's meaning ─────────────────────────────────────────────────────

fn abs(xs: &[&Value]) -> Option<Value> {
    number(num1(xs)?.abs())
}

// IEEE roundToIntegral in its three directions: exact, and `-0` kept (`Math.ceil(-0.5)` is `-0`).
fn floor(xs: &[&Value]) -> Option<Value> {
    number(num1(xs)?.floor())
}

fn ceil(xs: &[&Value]) -> Option<Value> {
    number(num1(xs)?.ceil())
}

fn trunc(xs: &[&Value]) -> Option<Value> {
    number(num1(xs)?.trunc())
}

/// `Math.round`: the integer closest to x, a tie toward +∞ (`Math.round(-2.5)` is -2), and `-0`
/// for x in [-0.5, -0]. `x - floor(x)` is exact for every double.
fn js_round(x: f64) -> f64 {
    let r = x.floor();
    let y = if x - r >= 0.5 { r + 1.0 } else { r };
    if y == 0.0 { 0.0f64.copysign(x) } else { y }
}

fn round(xs: &[&Value]) -> Option<Value> {
    number(js_round(num1(xs)?))
}

/// `Math.sign`: 1, -1, or x itself for `0` and `-0`.
fn sign(xs: &[&Value]) -> Option<Value> {
    let x = num1(xs)?;
    number(if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        x
    })
}

/// `Math.max(a, b)`: `+0` over `-0` (`f64::max` may give either).
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

/// `Math.min(a, b)`: `-0` under `+0`.
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

fn max(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number(js_max(a, b))
}

fn min(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number(js_min(a, b))
}

/// `Math.max(...xs)`: nothing for an empty list (JavaScript's `-Infinity`).
fn max_list(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    let mut it = items.iter();
    let &Value::Number(mut m) = it.next()? else { return None };
    for x in it {
        let Value::Number(x) = x else { return None };
        m = js_max(m, *x);
    }
    number(m)
}

/// ToUint32: truncate, then modulo 2^32 (`rem_euclid` is `%`, exact, and one exact addition).
fn to_uint32(x: f64) -> u32 {
    x.trunc().rem_euclid(4294967296.0) as u32
}

fn to_int32(x: f64) -> i32 {
    to_uint32(x) as i32
}

fn clz32(xs: &[&Value]) -> Option<Value> {
    number(to_uint32(num1(xs)?).leading_zeros() as f64)
}

fn imul(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number(to_int32(a).wrapping_mul(to_int32(b)) as f64)
}

/// `Math.fround`: IEEE conversion to binary32, round to nearest, ties to even; nothing past
/// float32's range (JavaScript's `±Infinity`).
fn fround(xs: &[&Value]) -> Option<Value> {
    number((num1(xs)? as f32) as f64)
}

/// `Math.f16round`, by the exponent: x's quantum is 2^(e-10) in float16's normal range and 2^-24
/// below it; x is divided by it (exact), rounded ties-to-even, and multiplied back (exact).
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

fn f16round(xs: &[&Value]) -> Option<Value> {
    number(f16_round(num1(xs)?))
}

/// V8's `Math.hypot` (builtins/math.tq): the largest magnitude found, each magnitude divided by it,
/// squared and summed with Kahan's compensation, then `sqrt(sum) * max`. IEEE operations only.
fn v8_hypot(values: &[f64]) -> (f64, f64) {
    let abs: Vec<f64> = values.iter().map(|x| x.abs()).collect();
    let mut m = 0.0;
    for &x in &abs {
        if x > m {
            m = x;
        }
    }
    if m == 0.0 {
        return (0.0, 0.0);
    }
    let (mut sum, mut comp) = (0.0f64, 0.0f64);
    for &x in &abs {
        let n = x / m;
        let summand = n * n - comp;
        let p = sum + summand;
        comp = (p - sum) - summand;
        sum = p;
    }
    (sum, m)
}

fn hypot(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    let (sum, m) = v8_hypot(&[a, b]);
    number(sum.sqrt() * m)
}

/// The sum V8's `Math.hypot(a, b)` takes the square root of, so the composite without its square
/// root can be checked.
fn hypot_sum(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number(v8_hypot(&[a, b]).0)
}

// `Number.isFinite`, `isNaN`, `isInteger`, `isSafeInteger`: no coercion, false for a value that
// isn't a number.
fn is_finite(xs: &[&Value]) -> Option<Value> {
    match xs {
        [Value::Number(x)] => boolean(x.is_finite()),
        [_] => boolean(false),
        _ => None,
    }
}

fn is_nan(xs: &[&Value]) -> Option<Value> {
    match xs {
        [Value::Number(x)] => boolean(x.is_nan()),
        [_] => boolean(false),
        _ => None,
    }
}

fn is_integer(xs: &[&Value]) -> Option<Value> {
    match xs {
        [Value::Number(x)] => boolean(x.is_finite() && x.trunc() == *x),
        [_] => boolean(false),
        _ => None,
    }
}

fn is_safe_integer(xs: &[&Value]) -> Option<Value> {
    match xs {
        [Value::Number(x)] => boolean(x.is_finite() && x.trunc() == *x && x.abs() <= 9007199254740991.0),
        [_] => boolean(false),
        _ => None,
    }
}

/// JavaScript's StrWhiteSpaceChar: WhiteSpace and LineTerminator (U+FEFF is one, U+0085 isn't,
/// unlike Rust's `char::is_whitespace`).
fn js_space(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// StrDecimalLiteral without `Infinity`: a sign, digits with an optional point, an exponent.
fn decimal_literal(t: &str) -> bool {
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
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

/// A `0x`, `0o` or `0b` literal's digits, rounded once to the nearest double: 120 bits kept, the
/// rest folded into a sticky bit.
fn radix_integer(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let (mut v, mut sticky, mut extra) = (0u128, false, 0i32);
    for c in digits.chars() {
        let Some(d) = c.to_digit(radix) else { return f64::NAN };
        if v >> 120 == 0 {
            v = v * radix as u128 + d as u128;
        } else {
            sticky |= d != 0;
            extra += radix.trailing_zeros() as i32;
        }
    }
    ((v | sticky as u128) as f64) * 2f64.powi(extra)
}

/// StringToNumber: trimmed; empty is 0; `Infinity`; `0x`/`0o`/`0b`; else a decimal literal.
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
    if decimal_literal(t) { t.parse().unwrap_or(f64::NAN) } else { f64::NAN }
}

/// ToNumber on a Holon value: a list reads as its `join(",")` (`Number([7])` is 7), a record as
/// `"[object Object]"`.
fn js_to_number(v: &Value) -> f64 {
    fn as_string(v: &Value) -> String {
        match v {
            Value::Number(x) if *x == 0.0 => "0".into(),
            Value::Number(x) => format!("{x:e}"),
            Value::Text(s) => s.clone(),
            Value::Boolean(b) => b.to_string(),
            Value::List(xs) => xs.iter().map(as_string).collect::<Vec<_>>().join(","),
            _ => "[object Object]".into(),
        }
    }
    match v {
        Value::Number(x) => *x,
        Value::Boolean(b) => *b as u8 as f64,
        Value::Text(s) => string_to_number(s),
        Value::List(_) => string_to_number(&as_string(v)),
        _ => f64::NAN,
    }
}

/// The global `isFinite(x)`: ToNumber first.
fn is_finite_global(xs: &[&Value]) -> Option<Value> {
    let [x] = xs else { return None };
    boolean(js_to_number(x).is_finite())
}

/// `a % b`: fmod, exact, the dividend's sign; nothing for `b` zero (JavaScript's `NaN`).
fn rem(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    if b == 0.0 { None } else { number(a % b) }
}

fn neg(xs: &[&Value]) -> Option<Value> {
    number(-num1(xs)?)
}

fn identity(xs: &[&Value]) -> Option<Value> {
    let [x] = xs else { return None };
    Some((*x).clone())
}

fn and32(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number((to_int32(a) & to_int32(b)) as f64)
}

fn or32(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number((to_int32(a) | to_int32(b)) as f64)
}

fn xor32(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number((to_int32(a) ^ to_int32(b)) as f64)
}

fn not32(xs: &[&Value]) -> Option<Value> {
    number(!to_int32(num1(xs)?) as f64)
}

fn shl(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number(to_int32(a).wrapping_shl(to_uint32(b) & 31) as f64)
}

fn shr(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number((to_int32(a) >> (to_uint32(b) & 31)) as f64)
}

fn ushr(xs: &[&Value]) -> Option<Value> {
    let (a, b) = num2(xs)?;
    number((to_uint32(a) >> (to_uint32(b) & 31)) as f64)
}

// The binary candidates with their second operand fixed, for the mapped races.
fn with(f: Native, xs: &[&Value], k: f64) -> Option<Value> {
    f(&[xs.first()?, &Value::Number(k)])
}
fn max_k(xs: &[&Value]) -> Option<Value> {
    with(max, xs, 200.0)
}
fn min_k(xs: &[&Value]) -> Option<Value> {
    with(min, xs, 200.0)
}
fn imul_k(xs: &[&Value]) -> Option<Value> {
    with(imul, xs, 2654435761.0)
}
fn rem_k(xs: &[&Value]) -> Option<Value> {
    with(rem, xs, 7.0)
}
fn and_k(xs: &[&Value]) -> Option<Value> {
    with(and32, xs, 1431655765.0)
}
fn or_k(xs: &[&Value]) -> Option<Value> {
    with(or32, xs, 1431655765.0)
}
fn xor_k(xs: &[&Value]) -> Option<Value> {
    with(xor32, xs, 1431655765.0)
}
fn shl_k(xs: &[&Value]) -> Option<Value> {
    with(shl, xs, 3.0)
}
fn shr_k(xs: &[&Value]) -> Option<Value> {
    with(shr, xs, 3.0)
}
fn ushr_k(xs: &[&Value]) -> Option<Value> {
    with(ushr, xs, 3.0)
}
fn hypot_k(xs: &[&Value]) -> Option<Value> {
    with(hypot, xs, 3.0)
}

/// `floor` reached through a `math {fn}` family leaf: its `fn` param found, then matched among the
/// family's names, before the same body. Raced against `floor` to price the family's dispatch.
fn floor_family(xs: &[&Value]) -> Option<Value> {
    let params: &[(&str, &str)] = black_box(&[("type", "Number"), ("fn", "floor")]);
    let f = params.iter().find(|(k, _)| *k == "fn").map(|(_, v)| *v)?;
    let x = num1(xs)?;
    number(match f {
        "abs" => x.abs(),
        "acos" | "acosh" | "asin" | "asinh" | "atan" | "atanh" | "cbrt" | "cos" | "cosh" | "exp" | "expm1" => x,
        "ceil" => x.ceil(),
        "clz32" => to_uint32(x).leading_zeros() as f64,
        "f16round" => f16_round(x),
        "fround" => (x as f32) as f64,
        "log" | "log10" | "log1p" | "log2" => x,
        "round" => js_round(x),
        "sign" => x.signum(),
        "sin" | "sinh" | "tan" | "tanh" => x,
        "sqrt" => x.sqrt(),
        "trunc" => x.trunc(),
        "floor" => x.floor(),
        _ => return None,
    })
}

// ── composites ───────────────────────────────────────────────────────────────────────────────────

// Math.abs: `0 - x` at and below zero keeps `-0` out (`x * -1` would give `-0` for `0`).
const ABS: &str = "function abs(x) {
  return x <= 0 ? 0 - x : x
}";

// Rounding by IEEE addition: below 2^52, `x + 2^52 - 2^52` (or `x - 2^52 + 2^52` for x < 0) is x
// rounded to the nearest integer, ties to even, exactly; one comparison corrects it to a floor or
// a ceiling. At and above 2^52 every double is an integer.
const FLOOR: &str = "function floor(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496 || x === 0) return x
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return r > x ? r - 1 : r
}";

// `x * 0` is `-0` for a negative x: the sign a ceiling of (-1, 0) keeps.
const CEIL: &str = "function ceil(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496 || x === 0) return x
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  const c = r < x ? r + 1 : r
  return c === 0 ? x * 0 : c
}";

const TRUNC: &str = "function trunc(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496 || x === 0) return x
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  const t = x > 0 ? (r > x ? r - 1 : r) : (r < x ? r + 1 : r)
  return t === 0 ? x * 0 : t
}";

// The nearest integer, ties to even, then a tie that went down (x - r is exactly 0.5) goes up;
// [-0.5, 0] keeps x's sign of zero.
const ROUND: &str = "function round(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496) return x
  if (x <= 0 && x >= -0.5) return x * 0
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return x - r === 0.5 ? r + 1 : r
}";

const SIGN: &str = "function sign(x) {
  return x > 0 ? 1 : x < 0 ? -1 : x
}";

// `a - b * -0`: when a isn't below b it is a, except two zeros, where it is their max (`-0` only
// when both are). Subtraction, not `a + b * 0`: `add` folds from `+0`, so it gives `0` for
// `-0 + -0`, where IEEE and JavaScript give `-0`.
const MAX: &str = "function max(a, b) {
  return a < b ? b : a - b * -0
}";

// `-max(-a, -b)`, negating by `* -1` (exact).
const MIN: &str = "function min(a, b) {
  return a > b ? b : (a * -1 - b * 0) * -1
}";

const MAX_OF: &str = "import { head, tail } from \"holon\"
function max(a, b) {
  return a < b ? b : a - b * -0
}
function max_of(xs) {
  return tail(xs).reduce((m, x) => max(m, x), head(xs))
}";

// Projection: the fold of a one-leaf step, where a `max` leaf would be.
const MAX_OF_LEAF: &str = "import { head, tail } from \"holon\"
function max_of(xs) {
  return tail(xs).reduce((m, x) => m + x, head(xs))
}";

// ToUint32 and ToInt32: |x| modulo 2^32 (the quotient floored exactly, the product exact, the
// difference exact by Sterbenz), truncated, and the sign put back modulo 2^32.
const INT32: &str = "function pfloor(y) {
  const r = y + 4503599627370496 - 4503599627370496
  return r > y ? r - 1 : r
}
function uint32(x) {
  const a = x < 0 ? 0 - x : x
  const q = a / 4294967296
  const m = a - (q >= 4503599627370496 ? q : pfloor(q)) * 4294967296
  const t = pfloor(m)
  return x < 0 && t > 0 ? 4294967296 - t : t
}
function int32(x) {
  const u = uint32(x)
  return u >= 2147483648 ? u - 4294967296 : u
}";

const CLZ32: &str = "function clz32(x) {
  let v = uint32(x)
  if (v === 0) return 32
  let n = 0
  if (v < 65536) {
    n = n + 16
    v = v * 65536
  }
  if (v < 16777216) {
    n = n + 8
    v = v * 256
  }
  if (v < 268435456) {
    n = n + 4
    v = v * 16
  }
  if (v < 1073741824) {
    n = n + 2
    v = v * 4
  }
  if (v < 2147483648) {
    n = n + 1
  }
  return n
}";

// The product modulo 2^32 from 16-bit halves, each partial product below 2^53 so exact.
const IMUL: &str = "function imul(a, b) {
  const x = uint32(a)
  const y = uint32(b)
  const xh = pfloor(x / 65536)
  const xl = x - xh * 65536
  const yh = pfloor(y / 65536)
  const yl = y - yh * 65536
  const mid = xh * yl + xl * yh
  const lo = mid - pfloor(mid / 65536) * 65536
  const p = lo * 65536 + xl * yl
  const u = p - pfloor(p / 4294967296) * 4294967296
  return u >= 2147483648 ? u - 4294967296 : u
}";

// Veltkamp's split, `c - (c - x)` with c = x * (2^29 + 1), is x rounded to nearest on 24 bits, ties
// to even; below float32's normal range, adding 1.5 * 2^-97 rounds x to a multiple of 2^-149.
const FROUND: &str = "function fround(x) {
  if (x < 1.1754943508222875e-38 && x > -1.1754943508222875e-38) {
    const r = x + 9.466330862652142e-30 - 9.466330862652142e-30
    return r === 0 ? x * 0 : r
  }
  const c = x * 536870913
  return x < 3.4028235677973366e38 && x > -3.4028235677973366e38 ? c - (c - x) : undefined
}";

// The same on 11 bits (2^42 + 1), and multiples of 2^-24 below float16's normal range.
const F16ROUND: &str = "function f16round(x) {
  if (x < 0.00006103515625 && x > -0.00006103515625) {
    const r = x + 402653184 - 402653184
    return r === 0 ? x * 0 : r
  }
  const c = x * 4398046511105
  return x < 65520 && x > -65520 ? c - (c - x) : undefined
}";

// V8's hypot for two arguments, without its last `sqrt(sum) * max` (no leaf takes a square root):
// a lower bound on the composite once `sqrt` is a leaf. For two values Kahan's compensation stays 0.
const HYPOT_SUM: &str = "function hypot_sum(a, b) {
  const x = a <= 0 ? 0 - a : a
  const y = b <= 0 ? 0 - b : b
  const m = x < y ? y : x
  if (m === 0) return 0
  const p = x / m
  const q = y / m
  return p * p + q * q
}";

const NEG: &str = "function neg(x) {
  return x * -1
}";

const NEG_SUBTRACT: &str = "function neg(x) {
  return 0 - x
}";

// One `default` leaf, the identity on a present value: the least a function can place.
const ONE: &str = "function one(x) {
  return x ?? 0
}";

// A comparison answers nothing for a value that isn't a number: `?? false` makes that false.
const IS_FINITE: &str = "function is_finite(x) {
  return (x <= x) ?? false
}";

// Holon has no NaN, so this is false for every value; `x !== x` is also JavaScript's own test, true
// only for NaN, should one ever be made.
const IS_NAN: &str = "function is_nan(x) {
  return x !== x
}";

const IS_INTEGER: &str = "function is_integer(x) {
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return (x >= 4503599627370496 || x <= -4503599627370496 || r === x) ?? false
}";

const IS_SAFE_INTEGER: &str = "function is_safe_integer(x) {
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return ((x >= 4503599627370496 || x <= -4503599627370496 || r === x) && x <= 9007199254740991 && x >= -9007199254740991) ?? false
}";

// Projection: `Number.isInteger(x)` as `trunc(x) === x` once `trunc` is a leaf; `x ?? 0` (one
// `default` leaf, the identity on a present x) stands where `trunc(x)` would.
const IS_INTEGER_LEAF: &str = "function is_integer(x) {
  return ((x ?? 0) === x) ?? false
}";

// Projection: `Math.ceil(x)` as `-floor(-x)` once `floor` is a leaf, `?? 0` standing for it.
const CEIL_LEAF: &str = "function ceil(x) {
  return ((x * -1) ?? 0) * -1
}";

// The global `isFinite(s)` once `to_number` is a leaf, `json_parse` standing for it: checked on
// texts both read alike.
const IS_FINITE_TEXT: &str = "import { json_parse } from \"holon\"
function is_finite_text(s) {
  const n = json_parse(s)
  return (n <= n) ?? false
}";

// `a % b` exactly: |b| scaled by a ladder of powers of two to the largest d = |b| * 2^k not above
// |a|, then k + 1 steps of `r >= d ? r - d : r` (exact by Sterbenz, as d <= r < 2d) with d halved
// (exact: every d is |b| times a power of two). The dividend's sign goes back on by `* -1`. The
// step is written in the lambda: a function returning a record literal is lowered with one output
// per field, so a fold through it would carry only its first field. The reader refuses a fold item
// the step doesn't use, so the step keeps the count as a field (one `with_field` a step).
const REM: &str = "import { range } from \"holon\"
function rem_pos(a, y) {
  const x = a < 0 ? 0 - a : a
  if (x < y) return a
  let d = y
  let k = 0
  if (d * 1.0715086071862673e301 <= x) {
    d = d * 1.0715086071862673e301
    k = k + 1000
  }
  if (d * 1.0715086071862673e301 <= x) {
    d = d * 1.0715086071862673e301
    k = k + 1000
  }
  if (d * 1.3407807929942597e154 <= x) {
    d = d * 1.3407807929942597e154
    k = k + 512
  }
  if (d * 1.157920892373162e77 <= x) {
    d = d * 1.157920892373162e77
    k = k + 256
  }
  if (d * 3.402823669209385e38 <= x) {
    d = d * 3.402823669209385e38
    k = k + 128
  }
  if (d * 18446744073709551616 <= x) {
    d = d * 18446744073709551616
    k = k + 64
  }
  if (d * 4294967296 <= x) {
    d = d * 4294967296
    k = k + 32
  }
  if (d * 65536 <= x) {
    d = d * 65536
    k = k + 16
  }
  if (d * 256 <= x) {
    d = d * 256
    k = k + 8
  }
  if (d * 16 <= x) {
    d = d * 16
    k = k + 4
  }
  if (d * 4 <= x) {
    d = d * 4
    k = k + 2
  }
  if (d * 2 <= x) {
    d = d * 2
    k = k + 1
  }
  const s = range(k + 1).reduce((acc, step) => ({ r: acc.r >= acc.d ? acc.r - acc.d : acc.r, d: acc.d / 2, n: step }), { r: x, d: d, n: 0 })
  return a < 0 ? s.r * -1 : s.r
}
function rem(a, b) {
  const y = b < 0 ? 0 - b : b
  return y > 0 ? rem_pos(a, y) : undefined
}";

// The bitwise operators bit by bit: a `fold` over the 32 bits' values, each step halving both
// operands exactly and adding the bit's value where both bits are 1; then `a | b` is
// `a + b - (a & b)` and `a ^ b` is `a + b - 2 * (a & b)` on the uint32s, exactly. A lambda is one
// expression, so each step floors each half twice (`half` and `low`).
const BITS: &str = "function bit_values() {
  return [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728, 268435456, 536870912, 1073741824, 2147483648]
}
function half(x) {
  return pfloor(x / 2)
}
function low(x) {
  return x - pfloor(x / 2) * 2
}
function and_u(x, y) {
  return bit_values().reduce((acc, p) => ({ x: half(acc.x), y: half(acc.y), r: acc.r + low(acc.x) * low(acc.y) * p }), { x: x, y: y, r: 0 }).r
}
function signed(u) {
  return u >= 2147483648 ? u - 4294967296 : u
}
function and32(a, b) {
  return signed(and_u(uint32(a), uint32(b)))
}
function or32(a, b) {
  const x = uint32(a)
  const y = uint32(b)
  return signed(x + y - and_u(x, y))
}
function xor32(a, b) {
  const x = uint32(a)
  const y = uint32(b)
  return signed(x + y - 2 * and_u(x, y))
}";

const NOT32: &str = "function not32(x) {
  return -1 - int32(x)
}";

// The shifts: 2^(count & 31) by a ladder of five choices; `<<` wraps the exact product to int32,
// `>>` floors an exact quotient, `>>>` floors the uint32's.
const SHIFTS: &str = "function pow2(s) {
  let p = 1
  let k = s
  if (k >= 16) {
    p = p * 65536
    k = k - 16
  }
  if (k >= 8) {
    p = p * 256
    k = k - 8
  }
  if (k >= 4) {
    p = p * 16
    k = k - 4
  }
  if (k >= 2) {
    p = p * 4
    k = k - 2
  }
  if (k >= 1) {
    p = p * 2
  }
  return p
}
function count(b) {
  const s = uint32(b)
  return s - pfloor(s / 32) * 32
}
function shl(a, b) {
  return int32(int32(a) * pow2(count(b)))
}
function shr(a, b) {
  return floor(int32(a) / pow2(count(b)))
}
function ushr(a, b) {
  return pfloor(uint32(a) / pow2(count(b)))
}";

// ── the races ────────────────────────────────────────────────────────────────────────────────────

/// Check a one-input composite against its candidate, then race it as a call on `at` and mapped.
fn unary(name: &str, ts: &str, f: Native, cases: &[Vec<Value>], at: f64) {
    let c = function(ts, name, 1);
    check(name, &Side::Composite(&c), &Side::Native(f), cases);
    race(name, &Side::Composite(&c), &Side::Native(f), &[1], |_| vec![num(at)]);
    let m = mapped(ts, name);
    check(&format!("{name}, mapped"), &Side::Composite(&m), &Side::NativeMapped(f), &[vec![mixed(64)]]);
    race(&format!("{name}, mapped"), &Side::Composite(&m), &Side::NativeMapped(f), SIZES, |n| vec![mixed(n)]);
}

/// The same for a two-input composite: mapped with its second operand fixed at `k` (`fk` is the
/// candidate with it fixed).
#[allow(clippy::too_many_arguments)]
fn binary(name: &str, ts: &str, f: Native, cases: &[Vec<Value>], at: (f64, f64), k: f64, fk: Native) {
    let c = function(ts, name, 2);
    check(name, &Side::Composite(&c), &Side::Native(f), cases);
    race(name, &Side::Composite(&c), &Side::Native(f), &[1], |_| vec![num(at.0), num(at.1)]);
    let wrapper = format!("{ts}\n\nfunction each_{name}(xs) {{\n  return xs.map((x) => {name}(x, {k}))\n}}");
    let mut m = function(&wrapper, &format!("each_{name}"), 1);
    m.name = format!("xs.map((x) => {name}(x, {k}))");
    check(&format!("{name}, mapped"), &Side::Composite(&m), &Side::NativeMapped(fk), &[vec![mixed(64)]]);
    race(&format!("{name}, mapped"), &Side::Composite(&m), &Side::NativeMapped(fk), SIZES, |n| vec![mixed(n)]);
}

/// A projection: a composite with a one-leaf stand-in where a proposed leaf would go, timed only.
fn projection(name: &str, ts: &str, fname: &str, f: Native, at: f64) {
    let c = function(ts, fname, 1);
    race(name, &Side::Composite(&c), &Side::Native(f), &[1], |_| vec![num(at)]);
    let m = mapped(ts, fname);
    race(&format!("{name}, mapped"), &Side::Composite(&m), &Side::NativeMapped(f), SIZES, |n| vec![mixed(n)]);
}

fn main() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let cases = unary_cases(&mut rng);

    // The constants: a `const` of the literal JavaScript prints gives the constant's exact bits.
    println!("\n### constants\n\n| JavaScript | literal | bits |\n|---|---|---|");
    let constants: [(&str, &str, f64); 13] = [
        ("Math.E", "2.718281828459045", std::f64::consts::E),
        ("Math.LN10", "2.302585092994046", std::f64::consts::LN_10),
        ("Math.LN2", "0.6931471805599453", std::f64::consts::LN_2),
        ("Math.LOG10E", "0.4342944819032518", std::f64::consts::LOG10_E),
        ("Math.LOG2E", "1.4426950408889634", std::f64::consts::LOG2_E),
        ("Math.PI", "3.141592653589793", std::f64::consts::PI),
        ("Math.SQRT1_2", "0.7071067811865476", std::f64::consts::FRAC_1_SQRT_2),
        ("Math.SQRT2", "1.4142135623730951", std::f64::consts::SQRT_2),
        ("Number.EPSILON", "2.220446049250313e-16", f64::EPSILON),
        ("Number.MAX_SAFE_INTEGER", "9007199254740991", 9007199254740991.0),
        ("Number.MIN_SAFE_INTEGER", "-9007199254740991", -9007199254740991.0),
        ("Number.MAX_VALUE", "1.7976931348623157e308", f64::MAX),
        ("Number.MIN_VALUE", "5e-324", f64::from_bits(1)),
    ];
    for (js, lit, want) in constants {
        let c = function(&format!("function k() {{\n  return {lit}\n}}"), "k", 0);
        let got = c.call(&[]);
        assert!(matches!(got, Some(Value::Number(g)) if g.to_bits() == want.to_bits()), "{js}: {lit} reads as {got:?}");
        println!("| `{js}` | `{lit}` | `{:016x}` |", want.to_bits());
    }

    // Math: rounding, sign, magnitude.
    unary("abs", ABS, abs, &cases, -3.7);
    unary("floor", FLOOR, floor, &cases, -3.7);
    unary("ceil", CEIL, ceil, &cases, -3.7);
    unary("trunc", TRUNC, trunc, &cases, -3.7);
    unary("round", ROUND, round, &cases, -3.7);
    unary("sign", SIGN, sign, &cases, -3.7);
    unary("fround", FROUND, fround, &cases, -3.7);
    unary("f16round", F16ROUND, f16round, &cases, -3.7);

    // The price of a family leaf's dispatch: `floor` behind a `math {fn}` match against `floor`.
    race("math family dispatch", &Side::Native(floor_family), &Side::Native(floor), &[1], |_| vec![num(-3.7)]);
    race("math family dispatch, mapped", &Side::NativeMapped(floor_family), &Side::NativeMapped(floor), SIZES, |n| vec![mixed(n)]);

    // Math.max, Math.min with two arguments; Math.max(...xs).
    let signed_set = [0.0, -0.0, 1.0, -1.0, 2.5, -2.5, 3.0, 1e308, -1e308, 5e-324, -5e-324, f64::MAX];
    let max_cases = pairs(&signed_set, &mut rng, 1000);
    binary("max", MAX, max, &max_cases, (3.5, 7.25), 200.0, max_k);
    binary("min", MIN, min, &max_cases, (3.5, 7.25), 200.0, min_k);
    let max_of = function(MAX_OF, "max_of", 1);
    let lists: Vec<Vec<Value>> = vec![
        vec![v("[]")],
        vec![v("[3]")],
        vec![Value::List(vec![num(-0.0), num(0.0)])],
        vec![Value::List(vec![num(0.0), num(-0.0)])],
        vec![Value::List(vec![num(-0.0), num(-0.0)])],
        vec![Value::List(vec![num(-5.0), num(-0.0), num(-1e308)])],
        vec![mixed(64)],
    ];
    check("max_of", &Side::Composite(&max_of), &Side::Native(max_list), &lists);
    race("max_of", &Side::Composite(&max_of), &Side::Native(max_list), SIZES, |n| vec![mixed(n)]);
    let max_of_leaf = function(MAX_OF_LEAF, "max_of", 1);
    race("max_of over a max leaf (projection)", &Side::Composite(&max_of_leaf), &Side::Native(max_list), SIZES, |n| vec![mixed(n)]);

    // Math.clz32, Math.imul.
    unary("clz32", &format!("{INT32}\n{CLZ32}"), clz32, &cases, 12345.6);
    let int_set = [
        0.0, -0.0, 1.0, -1.0, 1.5, -1.5, 31.0, 32.0, 33.0, 2147483647.0, 2147483648.0, -2147483648.0, -2147483649.0, 4294967296.0,
        4294967301.0, 2654435761.0, 1e10, -1e10, 1e300, 5e-324,
    ];
    let int_cases = pairs(&int_set, &mut rng, 1000);
    binary("imul", &format!("{INT32}\n{IMUL}"), imul, &int_cases, (123456789.0, 2654435761.0), 2654435761.0, imul_k);

    // Math.hypot(a, b): the composite before its square root, a lower bound.
    let hyp_set = [0.0, -0.0, 3.0, -4.0, 1e-300, 1e300, 5e-324, f64::MAX, 1.0, 1.5];
    let hyp_cases = pairs(&hyp_set, &mut rng, 1000);
    let hs = function(HYPOT_SUM, "hypot_sum", 2);
    check("hypot_sum", &Side::Composite(&hs), &Side::Native(hypot_sum), &hyp_cases);
    race("hypot without its sqrt (lower bound)", &Side::Composite(&hs), &Side::Native(hypot), &[1], |_| vec![num(3.0), num(4.0)]);
    let wrapper = format!("{HYPOT_SUM}\n\nfunction each_hypot_sum(xs) {{\n  return xs.map((x) => hypot_sum(x, 3))\n}}");
    let mut hm = function(&wrapper, "each_hypot_sum", 1);
    hm.name = "xs.map((x) => hypot_sum(x, 3))".into();
    race("hypot without its sqrt (lower bound), mapped", &Side::Composite(&hm), &Side::NativeMapped(hypot_k), SIZES, |n| vec![mixed(n)]);

    // Number.isFinite, isNaN, isInteger, isSafeInteger: numbers, and values that aren't.
    let mut typed = cases.clone();
    typed.extend([vec![text("5")], vec![text("")], vec![Value::Boolean(true)], vec![v("[1]")], vec![v("{\"a\": 1}")]]);
    unary("is_finite", IS_FINITE, is_finite, &typed, 3.5);
    unary("is_nan", IS_NAN, is_nan, &typed, 3.5);
    unary("is_integer", IS_INTEGER, is_integer, &typed, 3.5);
    unary("is_safe_integer", IS_SAFE_INTEGER, is_safe_integer, &typed, 3.5);
    projection("is_integer over a trunc leaf (projection)", IS_INTEGER_LEAF, "is_integer", is_integer, 3.5);
    projection("ceil over a floor leaf (projection)", CEIL_LEAF, "ceil", ceil, -3.7);

    // The global isFinite(s), `json_parse` standing for `to_number`.
    let fin = function(IS_FINITE_TEXT, "is_finite_text", 1);
    let json_texts: Vec<Vec<Value>> = ["5", "-0", "0", "1.5e3", "-7.25", "1e999", "-1e999", "abc", "123456789012345678901234567890", "1e-400", "true"]
        .iter()
        .map(|s| vec![text(s)])
        .collect();
    check("isFinite(s)", &Side::Composite(&fin), &Side::Native(is_finite_global), &json_texts);
    race("isFinite(s) over a to_number leaf (projection)", &Side::Composite(&fin), &Side::Native(is_finite_global), &[1], |_| vec![text("1.5e3")]);
    // Where JavaScript's grammar is wider than JSON's, the stand-in and ToNumber part.
    for (s, js) in [(" 5 ", true), ("0x1F", true), (".5", true), ("+5", true), ("", true), ("Infinity", false)] {
        assert_eq!(output(&Side::Native(is_finite_global), &[text(s)]), Some(Value::Boolean(js)), "isFinite({s:?})");
    }
    assert_eq!(js_to_number(&text("0x200000000000018")), 144115188075855904.0, "a hex literal past 2^53 rounds once");

    // a % b.
    let rem_set = [0.0, -0.0, 1.0, -1.0, 5.5, -5.5, 2.0, -2.0, 7.0, 0.1, 0.01, 3.0, 1e308, -1e308, 5e-324, 1e-300, C52 + 2.0, 4294967296.0];
    let rem_cases = pairs(&rem_set, &mut rng, 600);
    binary("rem", REM, rem, &rem_cases, (4095.0, 7.0), 7.0, rem_k);

    // Unary minus: `x * -1` is exact; `0 - x` gives `0` for `0`, where `-0` is right.
    unary("neg", NEG, neg, &cases, -3.7);
    let sub = function(NEG_SUBTRACT, "neg", 1);
    assert!(!same(&output(&Side::Composite(&sub), &[num(0.0)]).expect("a number"), &num(-0.0)), "0 - x at 0");

    // What any composite call costs: a function of one leaf (`x ?? 0`, the identity on a number)
    // against a leaf. And `-x` as the reader places it, in the caller's frame: its two nodes
    // against one, both inside a function, so the call's own cost is on both sides.
    let one = function(ONE, "one", 1);
    check("one", &Side::Composite(&one), &Side::Native(identity), &cases);
    race("a one-leaf function (baseline)", &Side::Composite(&one), &Side::Native(identity), &[1], |_| vec![num(-3.7)]);
    let one_m = mapped(ONE, "one");
    race("a one-leaf function (baseline), mapped", &Side::Composite(&one_m), &Side::NativeMapped(identity), SIZES, |n| vec![mixed(n)]);
    let neg_c = function(NEG, "neg", 1);
    race("neg in frame: x * -1 against one leaf", &Side::Composite(&neg_c), &Side::Composite(&one), &[1], |_| vec![num(-3.7)]);
    let neg_m = mapped(NEG, "neg");
    race("neg in frame: x * -1 against one leaf, mapped", &Side::Composite(&neg_m), &Side::Composite(&one_m), SIZES, |n| vec![mixed(n)]);

    // The bitwise operators.
    binary("and32", &format!("{INT32}\n{BITS}"), and32, &int_cases, (123456789.0, 1431655765.0), 1431655765.0, and_k);
    binary("or32", &format!("{INT32}\n{BITS}"), or32, &int_cases, (123456789.0, 1431655765.0), 1431655765.0, or_k);
    binary("xor32", &format!("{INT32}\n{BITS}"), xor32, &int_cases, (123456789.0, 1431655765.0), 1431655765.0, xor_k);
    unary("not32", &format!("{INT32}\n{NOT32}"), not32, &cases, 123456789.0);
    let shift_set = [0.0, -0.0, 1.0, -1.0, 3.0, 31.0, 32.0, 33.0, 1.5, -1.5, 2147483647.0, -2147483648.0, 4294967327.0, 1e10, -1e10, 1e300];
    let shift_cases = pairs(&shift_set, &mut rng, 600);
    let shifts = format!("{FLOOR}\n{INT32}\n{SHIFTS}");
    binary("shl", &shifts, shl, &shift_cases, (123456789.0, 3.0), 3.0, shl_k);
    binary("shr", &shifts, shr, &shift_cases, (-123456789.0, 3.0), 3.0, shr_k);
    binary("ushr", &shifts, ushr, &shift_cases, (-123456789.0, 3.0), 3.0, ushr_k);
}
