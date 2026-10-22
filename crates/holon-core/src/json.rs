//! JSON ⇄ `Value` for `json_parse` / `json_stringify` (M1): request and response bodies.
//!
//! Hand-written rather than a dependency, so core stays at two dependencies and every byte of
//! the mapping is ours to keep deterministic. Strict RFC 8259, plus two refusals that keep a
//! parsed value unambiguous: duplicate object keys, and lone UTF-16 surrogates. Input is
//! untrusted (it arrives in request bodies), so nesting is capped at [`MAX_DEPTH`].
//!
//! The mapping: object ⇄ `Record`, array ⇄ `List`, string ⇄ `Text`, number ⇄ `Number`,
//! `true`/`false` ⇄ `Boolean`, and `null` ⇄ `none()`. Stringifying `some(x)` writes `x`. An
//! `Effect`, a non-Option sum, or a non-finite number has no JSON form, so stringify refuses it.

use std::collections::BTreeMap;

use crate::Value;

/// How deeply arrays and objects may nest in parsed input.
pub const MAX_DEPTH: usize = 64;

/// `null`, as a value: the `none()` arm of an Option.
fn null() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}

/// Parse a JSON document. `None` for anything that isn't exactly one well-formed value
/// (surrounding whitespace allowed).
pub fn parse(text: &str) -> Option<Value> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    (p.i == p.s.len()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> Option<()> {
        self.s[self.i..].starts_with(lit.as_bytes()).then(|| self.i += lit.len())
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => self.object(depth + 1),
            b'[' => self.array(depth + 1),
            b'"' => self.string().map(Value::Text),
            b't' => self.eat("true").map(|_| Value::Boolean(true)),
            b'f' => self.eat("false").map(|_| Value::Boolean(false)),
            b'n' => self.eat("null").map(|_| null()),
            _ => self.number(),
        }
    }

    fn object(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.i += 1; // '{'
        let mut fields = BTreeMap::new();
        self.ws();
        if self.eat("}").is_some() {
            return Some(Value::Record(fields));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return None;
            }
            let key = self.string()?;
            self.ws();
            self.eat(":")?;
            let v = self.value(depth)?;
            if fields.insert(key, v).is_some() {
                return None; // a duplicate key: refuse rather than pick one
            }
            self.ws();
            match self.s.get(self.i)? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some(Value::Record(fields));
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.i += 1; // '['
        let mut items = Vec::new();
        self.ws();
        if self.eat("]").is_some() {
            return Some(Value::List(items));
        }
        loop {
            items.push(self.value(depth)?);
            self.ws();
            match self.s.get(self.i)? {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    return Some(Value::List(items));
                }
                _ => return None,
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            // Copy the longest run of plain bytes at once; it is valid UTF-8 because the input
            // is a &str and the run ends on an ASCII byte.
            let start = self.i;
            while matches!(self.s.get(self.i), Some(&b) if b != b'"' && b != b'\\' && b >= 0x20) {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).ok()?);
            match *self.s.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.i += 1;
                    let c = match *self.s.get(self.i)? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            self.i += 1;
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                self.eat("\\u")?;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code)?); // a lone low surrogate is None here
                            continue;
                        }
                        _ => return None,
                    };
                    self.i += 1;
                    out.push(c);
                }
                _ => return None, // a raw control character
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.s.get(self.i..self.i + 4)?;
        let text = std::str::from_utf8(digits).ok()?;
        if !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.i += 4;
        u32::from_str_radix(text, 16).ok()
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        let digits = |p: &mut Self| {
            let from = p.i;
            while matches!(p.s.get(p.i), Some(b'0'..=b'9')) {
                p.i += 1;
            }
            p.i > from
        };
        let _ = self.eat("-");
        if self.eat("0").is_none() && !digits(self) {
            return None; // no leading zeros, at least one digit
        }
        if self.eat(".").is_some() && !digits(self) {
            return None;
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return None;
            }
        }
        let n: f64 = std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()?;
        n.is_finite().then_some(Value::Number(n)) // 1e999 overflows to inf: refuse it
    }
}

/// Write `value` as compact JSON, object keys in order. `None` if anything in it has no JSON form.
pub fn stringify(value: &Value) -> Option<String> {
    let mut out = String::new();
    write(value, &mut out)?;
    Some(out)
}

fn write(v: &Value, out: &mut String) -> Option<()> {
    match v {
        Value::Number(n) if *n == 0.0 => out.push('0'), // `-0` too, as `to_text` prints it
        Value::Number(n) if n.is_finite() => out.push_str(&n.to_string()),
        Value::Number(_) => return None,
        Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Text(s) => write_string(s, out),
        Value::List(xs) => {
            out.push('[');
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out)?;
            }
            out.push(']');
        }
        Value::Record(fields) => {
            out.push('{');
            for (i, (k, x)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write(x, out)?;
            }
            out.push('}');
        }
        Value::Sum { arm: 0, payload } => write(payload, out)?,
        Value::Sum { arm: 1, payload } if **payload == Value::List(vec![]) => out.push_str("null"),
        Value::Sum { .. } | Value::Effect { .. } => return None,
    }
    Some(())
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pairs: &[(&str, Value)]) -> Value {
        Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }
    fn n(x: f64) -> Value {
        Value::Number(x)
    }
    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn parses_every_json_shape() {
        let v = parse(r#" {"id": 7, "title": "buy milk", "done": false, "tags": ["a", "b"], "due": null,
                          "n": -1.5e2, "esc": "q\"\\\/\n\u00e9\ud83d\ude00"} "#)
        .unwrap();
        assert_eq!(
            v,
            rec(&[
                ("id", n(7.0)),
                ("title", t("buy milk")),
                ("done", Value::Boolean(false)),
                ("tags", Value::List(vec![t("a"), t("b")])),
                ("due", null()),
                ("n", n(-150.0)),
                ("esc", t("q\"\\/\né😀")),
            ])
        );
        assert_eq!(parse("[]"), Some(Value::List(vec![])));
        assert_eq!(parse("{}"), Some(rec(&[])));
        assert_eq!(parse("0"), Some(n(0.0)));
        assert_eq!(parse("\"\""), Some(t("")));
    }

    #[test]
    fn refuses_malformed_and_ambiguous_input() {
        for bad in [
            "", " ", "{", "[1,]", "{\"a\":1,}", "01", "1.", ".5", "+1", "1e", "-", "NaN", "Infinity", "1e999",
            "'a'", "{a:1}", "\"\\x\"", "\"tab\there\"", "\"\\ud800\"", "\"\\udc00\"", "\"\\ud800\\u0041\"",
            "true false", "nul", "{\"a\":1,\"a\":2}", // duplicate key
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        // Nesting past the cap is refused instead of recursing without bound.
        let deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
        assert_eq!(parse(&deep), None);
        let ok = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_some());
    }

    #[test]
    fn stringifies_canonically_and_round_trips() {
        let v = rec(&[
            ("title", t("a \"quoted\"\nline\u{1}")),
            ("id", n(7.0)),
            ("ratio", n(0.1)),
            ("tags", Value::List(vec![t("x")])),
            ("due", null()),
            ("owner", Value::Sum { arm: 0, payload: Box::new(t("me")) }),
        ]);
        let s = stringify(&v).unwrap();
        assert_eq!(
            s,
            r#"{"due":null,"id":7,"owner":"me","ratio":0.1,"tags":["x"],"title":"a \"quoted\"\nline\u0001"}"#
        );
        // some(x) is written as x, so parsing it back gives x itself.
        let mut back = v.clone();
        if let Value::Record(f) = &mut back {
            f.insert("owner".into(), t("me"));
        }
        assert_eq!(parse(&s), Some(back));
    }

    #[test]
    fn refuses_values_with_no_json_form() {
        assert_eq!(stringify(&n(-0.0)).as_deref(), Some("0"), "-0 is 0, as the value hash has it");
        assert_eq!(stringify(&n(f64::NAN)), None);
        assert_eq!(stringify(&n(f64::INFINITY)), None);
        assert_eq!(stringify(&Value::Effect { sink: "egress".into(), payload: Box::new(n(1.0)) }), None);
        assert_eq!(stringify(&Value::Sum { arm: 2, payload: Box::new(n(1.0)) }), None);
        assert_eq!(stringify(&Value::List(vec![n(1.0), n(f64::NAN)])), None);
    }
}
