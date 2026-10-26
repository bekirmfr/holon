//! What a MongoDB call a flow sends asks of a collection (docs/L4s/databases-L4.md §3, slice 7).
//!
//! A MongoDB leaf's filter, update, document and sort are frozen params, as a SQL leaf's statement
//! is: **templates**, JSON whose values may be holes, `$1`, `$2`, …, filled from the leaf's
//! `params` list when it runs. The language writes them as record literals
//! (`notes.find("notes", { owner: who })`) and the lowering freezes their shape: every key and
//! operator is the flow's, every computed value a hole. So data can never add an operator: a
//! value that is a record with a key starting with `$` is refused where it is filled in (the host),
//! as a value can never change what a SQL statement does.
//!
//! - **What a template is.** `null`, `true`/`false`, numbers and strings as JSON writes them,
//!   lists, objects (their keys in the order written: a sort, and a compound `_id`, mean their
//!   order), and holes. Its text ([`Tpl::text`]) is compact JSON with holes bare: `{"owner":$1}`.
//! - **Refused,** each saying why: an operator that runs code on the server (`$where`,
//!   `$function`, `$accumulator`) anywhere; an update that isn't all update operators (`$set`,
//!   `$inc`, …; a replacement document or a pipeline isn't one); a document whose top-level key is
//!   an operator; a sort that isn't fields each `1` or `-1`, or has a hole; holes that skip a
//!   number.
//! - **A collection's name** is letters, digits, `_`, `-` and `.`, not `system.…` (the server's)
//!   nor `holon_…` (the host's own: where commits are marked and claims held).

use std::collections::BTreeSet;

/// A template: a JSON value whose values may be holes.
#[derive(Clone, Debug, PartialEq)]
pub enum Tpl {
    Null,
    Bool(bool),
    /// As written: `5`, `-1`, `2.5`.
    Number(String),
    Text(String),
    /// `$n`: the leaf's `n`th parameter (from 1).
    Hole(usize),
    List(Vec<Tpl>),
    /// Its fields in the order written.
    Object(Vec<(String, Tpl)>),
}

/// Which part of a call a template is: each is checked for what that part may say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// What a find, an update or a delete selects.
    Filter,
    /// What an update changes: update operators only.
    Update,
    /// What an insert adds.
    Document,
    /// The order a find answers in: fields, each `1` or `-1`, no holes.
    Sort,
}

/// Operators that run code on the server: never in a flow's template.
const CODE: &[&str] = &["$where", "$function", "$accumulator"];

/// What an update may do: the update operators.
const UPDATES: &[&str] = &["$set", "$unset", "$inc", "$mul", "$min", "$max", "$rename", "$setOnInsert", "$currentDate", "$push", "$pull", "$pullAll", "$addToSet", "$pop"];

impl Tpl {
    /// The template's text: compact JSON, holes bare (`{"owner":$1}`), its fields in their order.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Tpl::Null => out.push_str("null"),
            Tpl::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Tpl::Number(n) => out.push_str(n),
            Tpl::Text(t) => quote(t, out),
            Tpl::Hole(n) => out.push_str(&format!("${n}")),
            Tpl::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Tpl::Object(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// The holes it has, by number.
    pub fn holes(&self) -> BTreeSet<usize> {
        let mut found = BTreeSet::new();
        self.walk(&mut |t| {
            if let Tpl::Hole(n) = t {
                found.insert(*n);
            }
        });
        found
    }

    fn walk(&self, f: &mut impl FnMut(&Tpl)) {
        f(self);
        match self {
            Tpl::List(items) => items.iter().for_each(|i| i.walk(f)),
            Tpl::Object(fields) => fields.iter().for_each(|(_, v)| v.walk(f)),
            _ => {}
        }
    }

    /// Each key of each object in it.
    fn keys(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        fn go<'t>(t: &'t Tpl, keys: &mut Vec<&'t str>) {
            match t {
                Tpl::List(items) => items.iter().for_each(|i| go(i, keys)),
                Tpl::Object(fields) => {
                    for (k, v) in fields {
                        keys.push(k);
                        go(v, keys);
                    }
                }
                _ => {}
            }
        }
        go(self, &mut keys);
        keys
    }

    /// An object's field.
    pub fn get(&self, key: &str) -> Option<&Tpl> {
        match self {
            Tpl::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

fn quote(t: &str, out: &mut String) {
    out.push('"');
    for c in t.chars() {
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

/// Read a template's text.
pub fn parse(text: &str) -> Result<Tpl, String> {
    let mut p = Parser { s: text.as_bytes(), at: 0, text };
    let t = p.value(0)?;
    p.space();
    if p.at != p.s.len() {
        return Err(format!("more after the template, at {}", p.at));
    }
    Ok(t)
}

struct Parser<'t> {
    s: &'t [u8],
    at: usize,
    text: &'t str,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.at < self.s.len() && self.s[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Tpl, String> {
        if depth > 32 {
            return Err("a template nests more than 32 deep".into());
        }
        self.space();
        let Some(&c) = self.s.get(self.at) else { return Err("a template ends where a value should be".into()) };
        match c {
            b'{' => {
                self.at += 1;
                let mut fields: Vec<(String, Tpl)> = Vec::new();
                self.space();
                if self.s.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Tpl::Object(fields));
                }
                loop {
                    self.space();
                    if self.s.get(self.at) != Some(&b'"') {
                        return Err(format!("a key is a string, at {}", self.at));
                    }
                    let key = self.string()?;
                    if fields.iter().any(|(k, _)| *k == key) {
                        return Err(format!("the key {key:?} is given twice"));
                    }
                    self.space();
                    if self.s.get(self.at) != Some(&b':') {
                        return Err(format!("a key is followed by ':', at {}", self.at));
                    }
                    self.at += 1;
                    let v = self.value(depth + 1)?;
                    fields.push((key, v));
                    self.space();
                    match self.s.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Tpl::Object(fields));
                        }
                        _ => return Err(format!("an object goes on with ',' or ends with '}}', at {}", self.at)),
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.s.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Tpl::List(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.s.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Tpl::List(items));
                        }
                        _ => return Err(format!("a list goes on with ',' or ends with ']', at {}", self.at)),
                    }
                }
            }
            b'"' => Ok(Tpl::Text(self.string()?)),
            b'$' => {
                self.at += 1;
                let start = self.at;
                while self.at < self.s.len() && self.s[self.at].is_ascii_digit() {
                    self.at += 1;
                }
                let n: usize = self.text[start..self.at].parse().map_err(|_| format!("a hole is $ and its number, at {}", start - 1))?;
                if n == 0 || n > 100 {
                    return Err(format!("${n} isn't a hole: they are $1 to $100"));
                }
                Ok(Tpl::Hole(n))
            }
            b't' if self.text[self.at..].starts_with("true") => {
                self.at += 4;
                Ok(Tpl::Bool(true))
            }
            b'f' if self.text[self.at..].starts_with("false") => {
                self.at += 5;
                Ok(Tpl::Bool(false))
            }
            b'n' if self.text[self.at..].starts_with("null") => {
                self.at += 4;
                Ok(Tpl::Null)
            }
            b'-' | b'0'..=b'9' => {
                let start = self.at;
                self.at += 1;
                while self.at < self.s.len() && matches!(self.s[self.at], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    self.at += 1;
                }
                let n = &self.text[start..self.at];
                match n.parse::<f64>() {
                    Ok(v) if v.is_finite() => Ok(Tpl::Number(n.to_string())),
                    _ => Err(format!("{n:?} isn't a number")),
                }
            }
            _ => Err(format!("a template has a value here, at {}", self.at)),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.text[self.at..].chars().next() else { return Err("a string isn't closed".into()) };
            self.at += c.len_utf8();
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(e) = self.text[self.at..].chars().next() else { return Err("a string isn't closed".into()) };
                    self.at += 1;
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'u' => {
                            let hex = self.text.get(self.at..self.at + 4).ok_or("a \\u escape has four hex digits")?;
                            let code = u32::from_str_radix(hex, 16).map_err(|_| "a \\u escape has four hex digits")?;
                            self.at += 4;
                            out.push(char::from_u32(code).ok_or("a \\u escape names no character")?);
                        }
                        other => return Err(format!("\\{other} isn't an escape")),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

/// Read `text` as `part` of a call, checked for what that part may say: the template, and the
/// holes it has (a call's templates number theirs together: [`holes_in_order`]).
pub fn check(part: Part, text: &str) -> Result<(Tpl, BTreeSet<usize>), String> {
    let t = parse(text)?;
    let Tpl::Object(fields) = &t else {
        return Err(match part {
            Part::Filter => "a filter is a record".into(),
            Part::Update => "an update is a record of update operators ($set, $inc, …)".into(),
            Part::Document => "a document is a record".into(),
            Part::Sort => "a sort is a record of fields, each 1 or -1".into(),
        });
    };
    if let Some(op) = t.keys().into_iter().find(|k| CODE.contains(k)) {
        return Err(format!("{op} runs code on the database's server: a flow's call never does"));
    }
    match part {
        Part::Filter => {}
        Part::Update => {
            if fields.is_empty() {
                return Err("an update changes something: { \"$set\": { … } }".into());
            }
            if let Some((k, _)) = fields.iter().find(|(k, _)| !UPDATES.contains(&k.as_str())) {
                return Err(format!("{k:?} isn't an update operator: an update is a record of $set, $inc, $push, … (a replacement document isn't one)"));
            }
        }
        Part::Document => {
            if let Some((k, _)) = fields.iter().find(|(k, _)| k.starts_with('$')) {
                return Err(format!("a document's fields are data: {k:?} is an operator"));
            }
        }
        Part::Sort => {
            if let Some((k, _)) = fields.iter().find(|(_, v)| !matches!(v, Tpl::Number(n) if n == "1" || n == "-1")) {
                return Err(format!("a sort's fields are each 1 or -1, written as they are: {k:?} isn't"));
            }
        }
    }
    if part == Part::Sort && !t.holes().is_empty() {
        return Err("a sort is the flow's: it has no holes".into());
    }
    let holes = t.holes();
    Ok((t, holes))
}

/// Whether the holes a call's templates have between them are `$1` to `$n` with none skipped,
/// as a SQL statement's are: `n`, the parameters the call takes.
pub fn holes_in_order(holes: &BTreeSet<usize>) -> Result<usize, String> {
    let params = holes.last().copied().unwrap_or(0);
    match (1..=params).find(|n| !holes.contains(n)) {
        Some(missing) => Err(format!("the call has holes up to ${params} but no ${missing}")),
        None => Ok(params),
    }
}

/// The value a filter gives `_id` by equality, if it does: `{ _id: v }`, `{ _id: { "$eq": v } }`
/// (and `v` as `{ "$oid": … }`): the one document it can select, which a claim holds.
pub fn pinned_id(filter: &Tpl) -> Option<&Tpl> {
    let id = filter.get("_id")?;
    let id = id.get("$eq").filter(|_| matches!(id, Tpl::Object(f) if f.len() == 1)).unwrap_or(id);
    match id {
        Tpl::Object(fields) if fields.len() == 1 && fields[0].0 == "$oid" => Some(id),
        Tpl::Object(fields) if fields.iter().any(|(k, _)| k.starts_with('$')) => None,
        Tpl::List(_) | Tpl::Null => None,
        other => Some(other),
    }
}

/// Whether `name` may be a flow's collection: letters, digits, `_`, `-` and `.`, up to 120, not the
/// server's (`system.…`) nor the host's (`holon_…`).
pub fn is_collection_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && !name.starts_with('.')
        && !name.starts_with("system.")
        && !name.to_ascii_lowercase().starts_with("holon_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_reads_and_prints_as_written_its_holes_bare() {
        let text = r#"{"owner":$1,"n":{"$gt":5,"$lt":-2.5},"tags":["a",$2],"ok":true,"gone":null}"#;
        let t = parse(text).unwrap();
        assert_eq!(t.text(), text);
        assert_eq!(t.holes(), BTreeSet::from([1, 2]));
        assert_eq!(parse(" { \"a\" : \"q\\\"\\u00e9\" } ").unwrap().text(), "{\"a\":\"q\\\"é\"}");
        assert!(parse("{\"a\":$0}").is_err() && parse("{a:1}").is_err() && parse("{\"a\":1,\"a\":2}").is_err() && parse("{\"a\":1} x").is_err());
    }

    #[test]
    fn each_part_says_only_what_it_may() {
        assert_eq!(check(Part::Filter, r#"{"owner":$1,"at":{"$gt":$2}}"#).unwrap().1, BTreeSet::from([1, 2]));
        assert!(check(Part::Filter, r#"{"$where":"1"}"#).unwrap_err().contains("runs code"));
        assert!(check(Part::Filter, r#"{"a":{"$expr":{"$function":{}}}}"#).unwrap_err().contains("$function"));
        assert_eq!(holes_in_order(&BTreeSet::from([1, 2])), Ok(2));
        assert!(holes_in_order(&BTreeSet::from([2])).unwrap_err().contains("no $1"));
        assert!(check(Part::Filter, "[]").is_err());
        assert!(check(Part::Update, r#"{"$set":{"text":$1},"$inc":{"n":1}}"#).is_ok());
        assert!(check(Part::Update, r#"{"text":$1}"#).unwrap_err().contains("isn't an update operator"));
        assert!(check(Part::Update, "{}").is_err());
        assert!(check(Part::Document, r#"{"$set":1}"#).is_err() && check(Part::Document, r#"{"_id":$1,"a":{"b":2}}"#).is_ok());
        assert!(check(Part::Sort, r#"{"at":-1,"name":1}"#).is_ok());
        assert!(check(Part::Sort, r#"{"at":$1}"#).is_err() && check(Part::Sort, r#"{"at":2}"#).is_err());
    }

    #[test]
    fn a_filter_pins_the_document_its_id_equals() {
        let pin = |text: &str| pinned_id(&parse(text).unwrap()).map(Tpl::text);
        assert_eq!(pin(r#"{"_id":$1}"#).as_deref(), Some("$1"));
        assert_eq!(pin(r#"{"_id":{"$eq":"a"},"x":1}"#).as_deref(), Some("\"a\""));
        assert_eq!(pin(r#"{"_id":{"$oid":$1}}"#).as_deref(), Some("{\"$oid\":$1}"));
        assert_eq!(pin(r#"{"_id":{"$in":[$1]}}"#), None);
        assert_eq!(pin(r#"{"owner":$1}"#), None);
    }

    #[test]
    fn a_collection_is_a_flows_name_never_the_servers_or_the_hosts() {
        assert!(is_collection_name("notes") && is_collection_name("app.notes-2"));
        assert!(!is_collection_name("system.users") && !is_collection_name("holon_commits") && !is_collection_name("a b") && !is_collection_name(""));
    }
}
