//! Exact decimals over text (host-capabilities L4 §5.1.1): amounts as a payment file writes them,
//! `"1234.56"`, added, multiplied, divided, rounded and compared without the binary fractions a
//! number would round them to. A decimal is a text (or a finite number, as its shortest text), and
//! every result is one, written plainly. Each leaf gives nothing for anything else.

use crate::numbers::js_string;
use crate::value::Value;
use std::cmp::Ordering;

/// The most digits an input or a result may have, before the point and after: so a product of two
/// is bounded, as R1 asks.
pub(crate) const DECIMAL_DIGITS: usize = 1000;

/// `rounding`'s values, its family: Java's and decimal.js's seven.
pub(crate) const ROUNDINGS: &[&str] = &["half_even", "half_up", "half_down", "up", "down", "ceiling", "floor"];

/// A decimal: its sign, its digits as a whole number (least significant first, no zeros at the
/// top, none at all for zero) and how many of them are after the point.
#[derive(Clone, Debug, PartialEq)]
struct Dec {
    neg: bool,
    digits: Vec<u8>,
    scale: usize,
}

impl Dec {
    /// Its digits trimmed and a zero made positive: every result passes here.
    fn new(neg: bool, mut digits: Vec<u8>, scale: usize) -> Dec {
        while digits.last() == Some(&0) {
            digits.pop();
        }
        Dec { neg: neg && !digits.is_empty(), digits, scale }
    }

    /// How many digits it is written with: those before the point (one at least) and its scale.
    fn written(&self) -> usize {
        self.digits.len().saturating_sub(self.scale).max(1) + self.scale
    }

    /// The decimal itself, if it is within [`DECIMAL_DIGITS`].
    fn bounded(self) -> Option<Dec> {
        (self.written() <= DECIMAL_DIGITS).then_some(self)
    }

    /// Its digits with `n` zeros below them: the same number at `n` more places.
    fn widened(&self, n: usize) -> Vec<u8> {
        if self.digits.is_empty() {
            return vec![];
        }
        let mut d = vec![0; n];
        d.extend_from_slice(&self.digits);
        d
    }

    /// Plainly: no exponent, a digit before the point, `-` only for a negative.
    fn text(&self) -> String {
        let mut d: Vec<u8> = self.digits.clone();
        d.resize(d.len().max(self.scale + 1), 0);
        let mut s = String::with_capacity(d.len() + 2);
        if self.neg {
            s.push('-');
        }
        for (i, x) in d.iter().enumerate().rev() {
            s.push(char::from(b'0' + x));
            if i == self.scale && i > 0 {
                s.push('.');
            }
        }
        s
    }
}

/// A decimal from a text as `Number(…)` reads one (a sign, digits, a point, an exponent; no
/// spaces, `Infinity` or hex), or from a finite number as its shortest text.
fn read(v: &Value) -> Option<Dec> {
    match v {
        Value::Text(s) => parse(s),
        Value::Number(x) if x.is_finite() => parse(&js_string(v)),
        _ => None,
    }
}

fn parse(s: &str) -> Option<Dec> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int = &b[int_start..i];
    let mut frac: &[u8] = &[];
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac = &b[start..i];
    }
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let mut exp: i64 = 0;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        let eneg = match b.get(i) {
            Some(b'-') => {
                i += 1;
                true
            }
            Some(b'+') => {
                i += 1;
                false
            }
            _ => false,
        };
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            // Past twice the bound, no exponent makes a decimal within it.
            exp = (exp * 10 + i64::from(b[i] - b'0')).min(4 * DECIMAL_DIGITS as i64);
            i += 1;
        }
        if i == start {
            return None;
        }
        if eneg {
            exp = -exp;
        }
    }
    if i != b.len() {
        return None;
    }
    // The coefficient's digits, least significant first, without the zeros at its top.
    let mut digits: Vec<u8> = int.iter().chain(frac).rev().map(|c| c - b'0').collect();
    while digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.len() > DECIMAL_DIGITS {
        return None;
    }
    let scale = frac.len() as i64 - exp;
    let dec = if scale < 0 {
        let up = usize::try_from(-scale).ok()?;
        if digits.len() + up > DECIMAL_DIGITS {
            return None;
        }
        Dec::new(neg, Dec { neg, digits, scale: 0 }.widened(up), 0)
    } else {
        Dec::new(neg, digits, usize::try_from(scale).ok()?)
    };
    dec.bounded()
}

/// Two magnitudes' order.
fn cmp_mag(a: &[u8], b: &[u8]) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.iter().rev().cmp(b.iter().rev()))
}

fn add_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry = 0;
    for i in 0..a.len().max(b.len()) {
        let s = a.get(i).copied().unwrap_or(0) + b.get(i).copied().unwrap_or(0) + carry;
        out.push(s % 10);
        carry = s / 10;
    }
    if carry > 0 {
        out.push(carry);
    }
    out
}

/// `a − b`, for `a` at least `b`.
fn sub_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0;
    for (i, &x) in a.iter().enumerate() {
        let y = b.get(i).copied().unwrap_or(0) + borrow;
        if x >= y {
            out.push(x - y);
            borrow = 0;
        } else {
            out.push(x + 10 - y);
            borrow = 1;
        }
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

fn mul_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    if a.is_empty() || b.is_empty() {
        return vec![];
    }
    let mut acc = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        for (j, &y) in b.iter().enumerate() {
            acc[i + j] += u32::from(x) * u32::from(y);
        }
        // Carried at each row, so no cell passes 9 + 81.
        let mut carry = 0;
        for cell in acc.iter_mut().skip(i) {
            let s = *cell + carry;
            *cell = s % 10;
            carry = s / 10;
        }
    }
    let mut out: Vec<u8> = acc.into_iter().map(|c| c as u8).collect();
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

/// `n ÷ d` and its remainder, for `d` not zero: long division, a digit at a time.
fn divmod_mag(n: &[u8], d: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut q = vec![0u8; n.len()];
    let mut r: Vec<u8> = Vec::new();
    for i in (0..n.len()).rev() {
        r.insert(0, n[i]);
        while r.last() == Some(&0) {
            r.pop();
        }
        let mut k = 0;
        while cmp_mag(&r, d) != Ordering::Less {
            r = sub_mag(&r, d);
            k += 1;
        }
        q[i] = k;
    }
    while q.last() == Some(&0) {
        q.pop();
    }
    (q, r)
}

/// Whether a quotient `q` whose remainder over `d` is `r` rounds away from zero, by `mode`.
fn rounds_up(mode: &str, neg: bool, q: &[u8], r: &[u8], d: &[u8]) -> Option<bool> {
    if !ROUNDINGS.contains(&mode) {
        return None;
    }
    if r.is_empty() {
        return Some(false);
    }
    let half = cmp_mag(&add_mag(r, r), d);
    Some(match mode {
        "up" => true,
        "down" => false,
        "ceiling" => !neg,
        "floor" => neg,
        "half_up" => half != Ordering::Less,
        "half_down" => half == Ordering::Greater,
        "half_even" => half == Ordering::Greater || (half == Ordering::Equal && q.first().is_some_and(|x| x % 2 == 1)),
        _ => return None,
    })
}

/// `q` rounded by its remainder `r` over `d`, at `scale` places.
fn rounded(mode: &str, neg: bool, q: Vec<u8>, r: &[u8], d: &[u8], scale: usize) -> Option<Dec> {
    let q = if rounds_up(mode, neg, &q, r, d)? { add_mag(&q, &[1]) } else { q };
    Dec::new(neg, q, scale).bounded()
}

/// A scale: a whole number from 0 to [`DECIMAL_DIGITS`].
fn scale_of(v: &Value) -> Option<usize> {
    match v {
        Value::Number(x) if x.fract() == 0.0 && (0.0..=DECIMAL_DIGITS as f64).contains(x) => Some(*x as usize),
        _ => None,
    }
}

fn sum2(a: &Dec, b: &Dec) -> Option<Dec> {
    let scale = a.scale.max(b.scale);
    let (x, y) = (a.widened(scale - a.scale), b.widened(scale - b.scale));
    let dec = if a.neg == b.neg {
        Dec::new(a.neg, add_mag(&x, &y), scale)
    } else {
        match cmp_mag(&x, &y) {
            Ordering::Less => Dec::new(b.neg, sub_mag(&y, &x), scale),
            _ => Dec::new(a.neg, sub_mag(&x, &y), scale),
        }
    };
    dec.bounded()
}

fn negated(d: Dec) -> Dec {
    Dec::new(!d.neg, d.digits, d.scale)
}

/// `decimal_add`: the exact sum, its scale the largest of theirs.
pub(crate) fn add(inputs: &[&Value]) -> Option<String> {
    let mut acc = Dec::new(false, vec![], 0);
    for v in inputs {
        acc = sum2(&acc, &read(v)?)?;
    }
    Some(acc.text())
}

/// `decimal_sub`: `a − b`.
pub(crate) fn sub(a: &Value, b: &Value) -> Option<String> {
    Some(sum2(&read(a)?, &negated(read(b)?))?.text())
}

/// `decimal_mul`: the exact product, its scale the sum of theirs.
pub(crate) fn mul(inputs: &[&Value]) -> Option<String> {
    let mut acc = Dec::new(false, vec![1], 0);
    for v in inputs {
        let x = read(v)?;
        acc = Dec::new(acc.neg != x.neg, mul_mag(&acc.digits, &x.digits), acc.scale + x.scale).bounded()?;
    }
    Some(acc.text())
}

/// `decimal_div`: `a ÷ b` to `scale` places, by `mode`; nothing for `b` zero.
pub(crate) fn div(a: &Value, b: &Value, scale: &Value, mode: &str) -> Option<String> {
    let (a, b, scale) = (read(a)?, read(b)?, scale_of(scale)?);
    if b.digits.is_empty() {
        return None;
    }
    // a ÷ b at `scale` places is (A × 10^(b's scale + scale)) ÷ (B × 10^(a's scale)), whole.
    let n = a.widened(b.scale + scale);
    let d = b.widened(a.scale);
    let (q, r) = divmod_mag(&n, &d);
    Some(rounded(mode, a.neg != b.neg, q, &r, &d, scale)?.text())
}

/// `decimal_round`: `a` at `scale` places, by `mode`, padded with zeros.
pub(crate) fn round(a: &Value, scale: &Value, mode: &str) -> Option<String> {
    let (a, scale) = (read(a)?, scale_of(scale)?);
    if !ROUNDINGS.contains(&mode) {
        return None;
    }
    if scale >= a.scale {
        return Some(Dec::new(a.neg, a.widened(scale - a.scale), scale).bounded()?.text());
    }
    let cut = a.scale - scale;
    let q = a.digits.get(cut..).map(<[u8]>::to_vec).unwrap_or_default();
    let mut r = a.digits[..cut.min(a.digits.len())].to_vec();
    while r.last() == Some(&0) {
        r.pop();
    }
    let mut d = vec![0; cut];
    d.push(1);
    Some(rounded(mode, a.neg, q, &r, &d, scale)?.text())
}

/// `decimal_sum`: a list's exact sum, `"0"` for none.
pub(crate) fn sum(list: &[Value]) -> Option<String> {
    add(&list.iter().collect::<Vec<_>>())
}

/// `decimal_compare`: −1, 0 or 1, by value.
pub(crate) fn compare(a: &Value, b: &Value) -> Option<f64> {
    let (a, b) = (read(a)?, read(b)?);
    let scale = a.scale.max(b.scale);
    let (x, y) = (a.widened(scale - a.scale), b.widened(scale - b.scale));
    // A zero is never negative, so a sign alone may decide.
    let order = match (a.neg, b.neg) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => cmp_mag(&x, &y),
        (true, true) => cmp_mag(&y, &x),
    };
    Some(match order {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn reads_what_number_reads_and_writes_it_plainly() {
        let w = |s: &str| parse(s).map(|d| d.text());
        assert_eq!(w("1234.56"), Some("1234.56".into()));
        assert_eq!(w("+5"), Some("5".into()));
        assert_eq!(w("-0.50"), Some("-0.50".into()));
        assert_eq!(w(".5"), Some("0.5".into()));
        assert_eq!(w("5."), Some("5".into()));
        assert_eq!(w("007.10"), Some("7.10".into()));
        assert_eq!(w("1e-8"), Some("0.00000001".into()));
        assert_eq!(w("1.5E3"), Some("1500".into()));
        assert_eq!(w("-0"), Some("0".into()), "no negative zero");
        assert_eq!(w("-0.00"), Some("0.00".into()));
        for bad in ["", "-", ".", "1.2.3", " 1", "1 ", "1e", "1e+", "Infinity", "NaN", "0x10", "1,5", "e5"] {
            assert_eq!(w(bad), None, "{bad:?}");
        }
        assert_eq!(w(&"9".repeat(1000)), Some("9".repeat(1000)));
        assert_eq!(w(&"9".repeat(1001)), None, "past the bound");
        assert_eq!(w("1e999999999999999999999"), None);
        assert_eq!(w("1e-999999999999999999999"), None);
        // A number is its shortest text, not the binary fraction under it.
        assert_eq!(read(&Value::Number(0.1)).map(|d| d.text()), Some("0.1".into()));
        assert_eq!(read(&Value::Number(1e21)).map(|d| d.text()), Some("1000000000000000000000".into()));
        assert_eq!(read(&Value::Number(f64::NAN)), None);
        assert_eq!(read(&Value::Boolean(true)), None);
    }

    #[test]
    fn adds_subtracts_and_multiplies_exactly() {
        assert_eq!(add(&[&t("0.1"), &t("0.2")]), Some("0.3".into()));
        assert_eq!(add(&[&t("1.10"), &t("2")]), Some("3.10".into()), "the larger scale");
        assert_eq!(add(&[&t("-5.25"), &t("5.25")]), Some("0.00".into()));
        assert_eq!(add(&[&t("-1"), &t("0.25")]), Some("-0.75".into()));
        assert_eq!(add(&[&t("999.99"), &t("0.01"), &Value::Number(1.0)]), Some("1001.00".into()));
        assert_eq!(add(&[&t("1"), &t("x")]), None);
        assert_eq!(sub(&t("10.00"), &t("0.01")), Some("9.99".into()));
        assert_eq!(sub(&t("0.01"), &t("10")), Some("-9.99".into()));
        assert_eq!(mul(&[&t("19.99"), &t("3")]), Some("59.97".into()));
        assert_eq!(mul(&[&t("0.0825"), &t("59.97")]), Some("4.947525".into()));
        assert_eq!(mul(&[&t("-1.5"), &t("-2")]), Some("3.0".into()));
        assert_eq!(mul(&[&t("-1.5"), &t("0")]), Some("0.0".into()));
        assert_eq!(mul(&[&t(&"9".repeat(600)), &t(&"9".repeat(600))]), None, "a product past the bound");
        assert_eq!(sum(&[t("0.10"), t("0.20"), Value::Number(0.3)]), Some("0.60".into()));
        assert_eq!(sum(&[]), Some("0".into()));
    }

    #[test]
    fn rounds_by_each_mode() {
        let r = |a: &str, s: f64, m: &str| round(&t(a), &Value::Number(s), m);
        // Each mode on 2.5, -2.5, 2.51 and 2.4 to no places.
        let cases = [
            ("half_even", ["2", "-2", "3", "2"]),
            ("half_up", ["3", "-3", "3", "2"]),
            ("half_down", ["2", "-2", "3", "2"]),
            ("up", ["3", "-3", "3", "3"]),
            ("down", ["2", "-2", "2", "2"]),
            ("ceiling", ["3", "-2", "3", "3"]),
            ("floor", ["2", "-3", "2", "2"]),
        ];
        for (mode, want) in cases {
            let got = ["2.5", "-2.5", "2.51", "2.4"].map(|a| r(a, 0.0, mode).unwrap());
            assert_eq!(got, want, "{mode}");
        }
        assert_eq!(r("3.5", 0.0, "half_even"), Some("4".into()));
        assert_eq!(r("4.947525", 2.0, "half_even"), Some("4.95".into()));
        assert_eq!(r("2.675", 2.0, "half_even"), Some("2.68".into()), "exact, where 2.675 as a number is under it");
        assert_eq!(r("5", 2.0, "half_even"), Some("5.00".into()), "padded");
        assert_eq!(r("0.004", 2.0, "half_even"), Some("0.00".into()));
        assert_eq!(r("-0.004", 2.0, "half_even"), Some("0.00".into()), "no negative zero");
        assert_eq!(r("-0.005", 2.0, "floor"), Some("-0.01".into()));
        assert_eq!(r("9.995", 2.0, "half_up"), Some("10.00".into()), "a carry");
        assert_eq!(r("0.5", 0.0, "half_even"), Some("0".into()));
        assert_eq!(r("1", 1.5, "half_even"), None, "a scale is whole");
        assert_eq!(r("1", -1.0, "half_even"), None);
        assert_eq!(r("1", 1001.0, "half_even"), None);
        assert_eq!(r("1", 2.0, "nearest"), None, "not a mode");
    }

    #[test]
    fn divides_to_a_scale() {
        let d = |a: &str, b: &str, s: f64, m: &str| div(&t(a), &t(b), &Value::Number(s), m);
        assert_eq!(d("10", "3", 2.0, "half_even"), Some("3.33".into()));
        assert_eq!(d("20", "3", 2.0, "half_even"), Some("6.67".into()));
        assert_eq!(d("-20", "3", 2.0, "down"), Some("-6.66".into()));
        assert_eq!(d("1", "8", 2.0, "half_even"), Some("0.12".into()), "0.125, half to even");
        assert_eq!(d("1", "8", 2.0, "half_up"), Some("0.13".into()));
        assert_eq!(d("100.00", "0.25", 0.0, "half_even"), Some("400".into()));
        assert_eq!(d("0.003", "1000", 4.0, "half_even"), Some("0.0000".into()));
        assert_eq!(d("1", "0", 2.0, "half_even"), None);
        assert_eq!(d("1", "0.00", 2.0, "half_even"), None);
        assert_eq!(d("7", "-2", 1.0, "half_even"), Some("-3.5".into()));
    }

    #[test]
    fn compares_by_value() {
        assert_eq!(compare(&t("1.10"), &t("1.1")), Some(0.0));
        assert_eq!(compare(&t("10.00"), &t("9.99")), Some(1.0));
        assert_eq!(compare(&t("-1"), &Value::Number(0.5)), Some(-1.0));
        assert_eq!(compare(&t("-0"), &t("0.000")), Some(0.0));
        assert_eq!(compare(&t("1"), &t("one")), None);
        let (big, small) = (t(&"9".repeat(1000)), t(&format!("-{}", "9".repeat(1000))));
        assert_eq!(compare(&big, &small), Some(1.0), "a difference past the bound still compares");
    }
}
