//! JSON ⇄ `Value` for `json_parse` / `json_stringify` (M1): request and response bodies.
//!
//! Hand-written rather than a dependency, so core keeps to the few it has (blake3, hmac, libm, sha2, smallvec) and every byte of
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
        // As JavaScript writes a number (`1e+21`), `-0` as `0`, as `to_text` prints them.
        Value::Number(n) if n.is_finite() => out.push_str(&crate::primitives::number_text(*n)),
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

/// How `JSON.stringify(v, keys, space)` lays a value out: the gap its `space` gives (`""` for
/// none), and the keys a replacer list keeps, in the list's order.
pub(crate) struct Layout<'a> {
    pub gap: &'a str,
    pub keys: Option<&'a [String]>,
}

/// The gap a `json_stringify`'s `indent` gives: its first 10 UTF-16 code units, as JavaScript takes
/// a text `space`, short of an astral character the tenth would split (a text can't hold half of
/// one). The reader writes a number `space` as that many spaces, at most 10.
pub(crate) fn gap(indent: &str) -> &str {
    let mut units = 0;
    for (i, c) in indent.char_indices() {
        units += c.len_utf16();
        if units > 10 {
            return &indent[..i];
        }
    }
    indent
}

/// The keys a replacer list keeps (JavaScript's PropertyList): its texts, each once, in its order.
/// An item that isn't a text is skipped; JavaScript converts a number, which Holon doesn't.
pub(crate) fn key_list(items: &[Value]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for k in items {
        if let Value::Text(k) = k
            && !keys.contains(k)
        {
            keys.push(k.clone());
        }
    }
    keys
}

/// `JSON.stringify(value, keys, space)`: ECMA-262's SerializeJSONObject and SerializeJSONArray.
/// With a gap, each member on a line of its own indented by one more gap, `": "` after a key, the
/// closing bracket at the outer indent, and an empty list or record `[]` or `{}`. With keys, every
/// record at every depth written with only the listed keys it has, in the list's order; lists
/// whole. With neither, [`stringify`]. Nothing once the text passes `MAX_TEXT` bytes: each member
/// is written after its depth's gap, so a body of many items nested deep would lay out to
/// gigabytes.
pub(crate) fn stringify_laid_out(value: &Value, layout: &Layout) -> Option<String> {
    if layout.gap.is_empty() && layout.keys.is_none() {
        return stringify(value);
    }
    let mut out = String::new();
    write_laid_out(value, layout, "", &mut out)?;
    Some(out)
}

fn write_laid_out(v: &Value, layout: &Layout, indent: &str, out: &mut String) -> Option<()> {
    match v {
        Value::Sum { arm: 0, payload } => write_laid_out(payload, layout, indent, out),
        Value::List(xs) => write_members(xs.iter().map(|x| (None, x)), ('[', ']'), layout, indent, out),
        Value::Record(fields) => match layout.keys {
            Some(keys) => write_members(keys.iter().filter_map(|k| fields.get(k).map(|x| (Some(k.as_str()), x))), ('{', '}'), layout, indent, out),
            None => write_members(fields.iter().map(|(k, x)| (Some(k.as_str()), x)), ('{', '}'), layout, indent, out),
        },
        _ => write(v, out),
    }
}

/// A list's items or a record's members, each laid out at one more gap.
fn write_members<'v>(members: impl Iterator<Item = (Option<&'v str>, &'v Value)>, (open, close): (char, char), layout: &Layout, indent: &str, out: &mut String) -> Option<()> {
    let inner = format!("{indent}{}", layout.gap);
    let laid = !layout.gap.is_empty();
    out.push(open);
    let mut any = false;
    for (key, x) in members {
        if any {
            out.push(',');
        }
        any = true;
        if laid {
            if out.len() > crate::texts::MAX_TEXT {
                return None;
            }
            out.push('\n');
            out.push_str(&inner);
        }
        if let Some(k) = key {
            write_string(k, out);
            out.push_str(if laid { ": " } else { ":" });
        }
        write_laid_out(x, layout, &inner, out)?;
    }
    if any && laid {
        out.push('\n');
        out.push_str(indent);
    }
    out.push(close);
    Some(())
}

/// `JSON.parse(text, reviver)` once the text is parsed: ECMA-262's InternalizeJSONProperty.
/// `revive` is applied to each member of a record or list before the record or list itself (a
/// list item's key its index as text), and last to the whole value with key `""`; what it answers
/// stands in for the member. A record member it answers nothing for is removed, and a list item
/// it answers nothing for is `none()`, JSON's `null` (JavaScript leaves a hole, which reads as
/// `undefined` and is written `null`); nothing for the whole value is nothing. JavaScript visits a
/// record's keys in the text's order and this in sorted order; the function is pure, so the order
/// can't be seen. An `Err` from `revive` stops the walk there.
pub(crate) fn revive<E>(value: Value, revive: &mut impl FnMut(&str, &Value) -> Result<Option<Value>, E>) -> Result<Option<Value>, E> {
    internalize("", value, revive)
}

fn internalize<E>(key: &str, value: Value, revive: &mut impl FnMut(&str, &Value) -> Result<Option<Value>, E>) -> Result<Option<Value>, E> {
    let value = match value {
        Value::List(xs) => {
            let mut items = Vec::with_capacity(xs.len());
            for (i, x) in xs.into_iter().enumerate() {
                items.push(internalize(&i.to_string(), x, revive)?.unwrap_or_else(null));
            }
            Value::List(items)
        }
        Value::Record(fields) => {
            let mut kept = BTreeMap::new();
            for (k, x) in fields {
                if let Some(x) = internalize(&k, x, revive)? {
                    kept.insert(k, x);
                }
            }
            Value::Record(kept)
        }
        v => v,
    };
    revive(key, &value)
}

/// How deep the lists and records a replacer answers may nest: as deep as a value read back may
/// (`value_hash`). One that wraps what it is given (`(k, v) => [k, v]`) would otherwise nest
/// without end, where JavaScript runs out of stack and throws.
pub(crate) const MAX_REPLACED_DEPTH: usize = crate::value_hash::MAX_VALUE_DEPTH;

/// Why a replacer's walk stopped: `replace` answered `Err`, or what it answered nested past
/// [`MAX_REPLACED_DEPTH`].
enum Stop<E> {
    Replacer(E),
    TooDeep,
}

/// `JSON.stringify(value, replacer)`'s value once replaced: ECMA-262's SerializeJSONProperty with a
/// replacer function, top-down. `replace` is applied first to the whole value with key `""`, then
/// to each member of what it answered (a list item's key its index as text). A record member it
/// answers nothing for is left out, and a list item it answers nothing for is `none()` (written
/// `null`); nothing for the whole value is nothing (JavaScript's `undefined`), as is an answer
/// nesting past [`MAX_REPLACED_DEPTH`] (JavaScript's RangeError). `some(x)` has `x`'s members, as
/// it is written as `x`. An `Err` from `replace` stops the walk there.
pub(crate) fn replace<E>(value: &Value, replace: &mut impl FnMut(&str, &Value) -> Result<Option<Value>, E>) -> Result<Option<Value>, E> {
    match serialize_property("", value, 0, replace) {
        Ok(v) => Ok(v),
        Err(Stop::Replacer(e)) => Err(e),
        Err(Stop::TooDeep) => Ok(None),
    }
}

fn serialize_property<E>(key: &str, value: &Value, depth: usize, replace: &mut impl FnMut(&str, &Value) -> Result<Option<Value>, E>) -> Result<Option<Value>, Stop<E>> {
    match replace(key, value).map_err(Stop::Replacer)? {
        Some(v) => replaced_members(v, depth, replace).map(Some),
        None => Ok(None),
    }
}

fn replaced_members<E>(value: Value, depth: usize, replace: &mut impl FnMut(&str, &Value) -> Result<Option<Value>, E>) -> Result<Value, Stop<E>> {
    if matches!(value, Value::List(_) | Value::Record(_)) && depth >= MAX_REPLACED_DEPTH {
        return Err(Stop::TooDeep);
    }
    Ok(match value {
        Value::Sum { arm: 0, payload } => replaced_members(*payload, depth, replace)?,
        Value::List(xs) => {
            let mut items = Vec::with_capacity(xs.len());
            for (i, x) in xs.iter().enumerate() {
                items.push(serialize_property(&i.to_string(), x, depth + 1, replace)?.unwrap_or_else(null));
            }
            Value::List(items)
        }
        Value::Record(fields) => {
            let mut kept = BTreeMap::new();
            for (k, x) in &fields {
                if let Some(x) = serialize_property(k, x, depth + 1, replace)? {
                    kept.insert(k.clone(), x);
                }
            }
            Value::Record(kept)
        }
        v => v,
    })
}

/// A text as JSON, escaped as JavaScript's QuoteJSONString escapes it: `\b`, `\f`, `\n`, `\r` and
/// `\t` by name, any other code point below U+0020 as `\u00xx` in lowercase hex.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
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

    /// As JavaScript's JSON.stringify: the five named escapes, the other control characters in
    /// lowercase hex, and the text read back as it was.
    #[test]
    fn a_control_character_is_escaped_as_javascript_escapes_it() {
        let s = "\u{8}\u{c}\n\r\t\u{0}\u{1f}\u{b}\"\\\u{7f}";
        let written = stringify(&t(s)).unwrap();
        assert_eq!(written, "\"\\b\\f\\n\\r\\t\\u0000\\u001f\\u000b\\\"\\\\\u{7f}\"");
        assert_eq!(parse(&written), Some(t(s)));
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

    /// The layouts the races pinned against what JavaScript writes for the same keys in the same
    /// order (races_records.rs): an indent's gap, and a key list at every depth.
    #[test]
    fn an_indent_and_a_key_list_lay_out_as_javascript_does() {
        let doc = parse(r#"{"a": "x", "b": [1, {}, []], "c": {"d": true}}"#).unwrap();
        let indented = |indent: &str| stringify_laid_out(&doc, &Layout { gap: gap(indent), keys: None }).unwrap();
        assert_eq!(indented("  "), "{\n  \"a\": \"x\",\n  \"b\": [\n    1,\n    {},\n    []\n  ],\n  \"c\": {\n    \"d\": true\n  }\n}");
        assert_eq!(indented("\t"), "{\n\t\"a\": \"x\",\n\t\"b\": [\n\t\t1,\n\t\t{},\n\t\t[]\n\t],\n\t\"c\": {\n\t\t\"d\": true\n\t}\n}");
        assert_eq!(indented(""), r#"{"a":"x","b":[1,{},[]],"c":{"d":true}}"#);
        assert_eq!(indented("abcdefghijkl").lines().nth(1), Some("abcdefghij\"a\": \"x\","));
        // Ten UTF-16 code units: an astral character the tenth would split is left out.
        assert_eq!(gap("abcdefghi😀"), "abcdefghi");
        assert_eq!(gap("abcdefgh😀z"), "abcdefgh😀");
        assert_eq!(stringify_laid_out(&n(1.5), &Layout { gap: "  ", keys: None }).as_deref(), Some("1.5"));
        let some = Value::Sum { arm: 0, payload: Box::new(Value::List(vec![n(1.0)])) };
        assert_eq!(stringify_laid_out(&some, &Layout { gap: " ", keys: None }).as_deref(), Some("[\n 1\n]"));
        let keyed = parse(r#"{"b": 1, "a": {"b": 2, "c": 3}, "d": [{"a": 4, "e": 5}]}"#).unwrap();
        let keys = |list: &str| {
            let Some(Value::List(items)) = parse(list) else { panic!("{list}") };
            let keys = key_list(&items);
            stringify_laid_out(&keyed, &Layout { gap: "", keys: Some(&keys) }).unwrap()
        };
        assert_eq!(keys(r#"["b", "a"]"#), r#"{"b":1,"a":{"b":2}}"#);
        assert_eq!(keys(r#"["d", "a", "a", 7]"#), r#"{"d":[{"a":4}],"a":{}}"#);
        assert_eq!(keys("[]"), "{}");
        let both = stringify_laid_out(&keyed, &Layout { gap: " ", keys: Some(&["a".to_string(), "x".to_string()]) }).unwrap();
        assert_eq!(both, "{\n \"a\": {}\n}");
    }

    /// The reviver is applied bottom-up and the replacer top-down, each member by its key (an
    /// item's index as text); what either answers nothing for is removed from a record and `null`
    /// in a list; an `Err` stops either where it is.
    #[test]
    fn a_reviver_walks_bottom_up_and_a_replacer_top_down() {
        let doc = parse(r#"{"a": [1, 2], "b": {"c": 3}}"#).unwrap();
        let mut seen = Vec::new();
        let doubled = revive(doc.clone(), &mut |k: &str, v: &Value| {
            seen.push(k.to_string());
            Ok::<_, ()>(match v {
                Value::Number(_) if k == "1" => None,
                Value::Number(x) => Some(n(x * 2.0)),
                _ if k == "b" => None,
                v => Some(v.clone()),
            })
        });
        assert_eq!(seen, ["0", "1", "a", "c", "b", ""]);
        assert_eq!(doubled, Ok(Some(parse(r#"{"a": [2, null]}"#).unwrap())));
        assert_eq!(revive(n(1.0), &mut |_: &str, _: &Value| Ok::<_, ()>(None)), Ok(None));
        assert_eq!(revive(doc.clone(), &mut |k: &str, v: &Value| if k == "c" { Err("stop") } else { Ok(Some(v.clone())) }), Err("stop"));

        let mut seen = Vec::new();
        let replaced = replace(&doc, &mut |k: &str, v: &Value| {
            seen.push(k.to_string());
            Ok::<_, ()>(match v {
                // The whole value answered with another record: its members are the new one's.
                _ if k.is_empty() => Some(rec(&[("a", parse("[1, 2]").unwrap()), ("x", n(0.0)), ("s", Value::Sum { arm: 0, payload: Box::new(rec(&[("t", n(5.0))])) })])),
                Value::Number(_) if k == "1" || k == "x" => None,
                v => Some(v.clone()),
            })
        });
        assert_eq!(seen, ["", "a", "0", "1", "s", "t", "x"]);
        assert_eq!(replaced, Ok(Some(parse(r#"{"a": [1, null], "s": {"t": 5}}"#).unwrap())));
        assert_eq!(replace(&doc, &mut |_: &str, _: &Value| Ok::<_, ()>(None)), Ok(None));
        assert_eq!(replace(&doc, &mut |k: &str, v: &Value| if k == "a" { Err("stop") } else { Ok(Some(v.clone())) }), Err("stop"));
    }

    /// Many items nested deep, laid out with a wide gap, grow by the gap at every depth for each
    /// item: past `MAX_TEXT` bytes the text is nothing, where a 4 MB body of them made 1.3 GB.
    #[test]
    fn a_laid_out_text_stops_at_the_bound() {
        let wide = (0..60).fold(Value::List(vec![n(0.0); 30_000]), |v, _| Value::List(vec![v]));
        assert_eq!(stringify_laid_out(&wide, &Layout { gap: "          ", keys: None }), None);
        assert!(stringify_laid_out(&wide, &Layout { gap: " ", keys: None }).is_some());
    }

    /// A replacer that wraps what it is given nests its answer without end: past
    /// `MAX_REPLACED_DEPTH` the whole value is nothing, on a thread a host's size, where it once
    /// ran out of stack. One that keeps what it is given still walks a value as deep as that.
    #[test]
    fn a_replacer_nesting_without_end_gives_nothing() {
        let wrapped = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| replace(&n(0.0), &mut |k: &str, v: &Value| Ok::<_, ()>(Some(Value::List(vec![Value::Text(k.into()), v.clone()])))))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(wrapped, Ok(None));
        let deep = (0..MAX_REPLACED_DEPTH).fold(n(1.0), |v, _| Value::List(vec![v]));
        assert_eq!(replace(&deep, &mut |_: &str, v: &Value| Ok::<_, ()>(Some(v.clone()))), Ok(Some(deep.clone())));
        let deeper = Value::List(vec![deep]);
        assert_eq!(replace(&deeper, &mut |_: &str, v: &Value| Ok::<_, ()>(Some(v.clone()))), Ok(None));
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
