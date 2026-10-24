//! One member of a JSON file set or removed in place, the rest of the file left as it was, byte
//! for byte: its order, its indentation, and the comments and trailing commas some hosts allow
//! (VS Code's and Zed's settings are JSON with comments). `holon mcp setup` edits other programs'
//! settings with it (mcphosts.rs), so a person who opens the file afterwards finds their own file,
//! with one entry more.

use serde_json::Value;

/// A value in the text: where it is, and for an object, its members.
#[derive(Debug)]
struct Node {
    start: usize,
    end: usize,
    object: Option<Object>,
}

#[derive(Debug)]
struct Object {
    /// Just past the `{`.
    open: usize,
    /// At the `}`.
    close: usize,
    members: Vec<Member>,
}

#[derive(Debug)]
struct Member {
    key: String,
    /// Where its key's `"` is.
    start: usize,
    value: Node,
    /// The `,` after it, if any.
    comma: Option<usize>,
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn fail<T>(&self, what: &str) -> Result<T, String> {
        let line = self.text[..self.at.min(self.text.len())].iter().filter(|&&b| b == b'\n').count() + 1;
        Err(format!("line {line}: {what}"))
    }

    /// Past whitespace and comments.
    fn skip(&mut self) -> Result<(), String> {
        loop {
            match self.text.get(self.at..self.at + 2) {
                Some(b"//") => {
                    while self.at < self.text.len() && self.text[self.at] != b'\n' {
                        self.at += 1;
                    }
                }
                Some(b"/*") => {
                    let rest = &self.text[self.at + 2..];
                    match rest.windows(2).position(|w| w == b"*/") {
                        Some(i) => self.at += 2 + i + 2,
                        None => return self.fail("a comment that never ends"),
                    }
                }
                _ => match self.text.get(self.at) {
                    Some(b' ' | b'\t' | b'\r' | b'\n') => self.at += 1,
                    _ => return Ok(()),
                },
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.at;
        self.at += 1;
        loop {
            match self.text.get(self.at) {
                None => return self.fail("a string that never ends"),
                Some(b'\\') => self.at += 2,
                Some(b'"') => {
                    self.at += 1;
                    let raw = std::str::from_utf8(&self.text[start..self.at]).map_err(|e| e.to_string())?;
                    return serde_json::from_str(raw).or_else(|e| self.fail(&format!("a string it can't read ({e})")));
                }
                Some(_) => self.at += 1,
            }
        }
    }

    fn value(&mut self) -> Result<Node, String> {
        self.skip()?;
        let start = self.at;
        match self.text.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                let open = self.at;
                let mut members = Vec::new();
                loop {
                    self.skip()?;
                    match self.text.get(self.at) {
                        Some(b'}') => break,
                        Some(b'"') => {}
                        _ => return self.fail("expected a key or `}`"),
                    }
                    let key_at = self.at;
                    let key = self.string()?;
                    self.skip()?;
                    if self.text.get(self.at) != Some(&b':') {
                        return self.fail("expected `:` after a key");
                    }
                    self.at += 1;
                    let value = self.value()?;
                    self.skip()?;
                    let comma = (self.text.get(self.at) == Some(&b',')).then_some(self.at);
                    members.push(Member { key, start: key_at, value, comma });
                    match self.text.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {}
                        _ => return self.fail("expected `,` or `}`"),
                    }
                }
                let close = self.at;
                self.at += 1;
                Ok(Node { start, end: self.at, object: Some(Object { open, close, members }) })
            }
            Some(b'[') => {
                self.at += 1;
                loop {
                    self.skip()?;
                    if self.text.get(self.at) == Some(&b']') {
                        break;
                    }
                    self.value()?;
                    self.skip()?;
                    match self.text.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {}
                        _ => return self.fail("expected `,` or `]`"),
                    }
                }
                self.at += 1;
                Ok(Node { start, end: self.at, object: None })
            }
            Some(b'"') => {
                self.string()?;
                Ok(Node { start, end: self.at, object: None })
            }
            Some(_) => {
                while self.at < self.text.len() && !matches!(self.text[self.at], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n' | b'/') {
                    self.at += 1;
                }
                let word = std::str::from_utf8(&self.text[start..self.at]).unwrap_or("");
                if serde_json::from_str::<Value>(word).is_err() {
                    self.at = start;
                    return self.fail(&format!("{word:?} isn't a JSON value"));
                }
                Ok(Node { start, end: self.at, object: None })
            }
            None => self.fail("it ends where a value should be"),
        }
    }
}

/// The text's top-level object (an empty text is an empty object).
fn parse(text: &str) -> Result<Node, String> {
    let mut p = Parser { text: text.as_bytes(), at: 0 };
    let node = p.value()?;
    p.skip()?;
    if p.at != text.len() {
        return p.fail("more after the end of the JSON");
    }
    if node.object.is_none() {
        return Err("it isn't a JSON object".into());
    }
    Ok(node)
}

/// `text[start..end]` as plain JSON: comments blanked, trailing commas dropped.
fn plain(text: &str, start: usize, end: usize) -> String {
    let mut p = Parser { text: text.as_bytes(), at: start };
    let mut out = String::new();
    while p.at < end {
        let before = p.at;
        if p.skip().is_ok() && p.at > before {
            out.push(' ');
            continue;
        }
        match text.as_bytes()[p.at] {
            b'"' => {
                let _ = p.string();
                out.push_str(&text[before..p.at]);
            }
            close @ (b']' | b'}') => {
                let kept = out.trim_end().len();
                out.truncate(kept);
                if out.ends_with(',') {
                    out.pop();
                }
                out.push(close as char);
                p.at += 1;
            }
            _ => {
                let c = text[p.at..].chars().next().unwrap();
                out.push(c);
                p.at += c.len_utf8();
            }
        }
    }
    out
}

/// The member at `path` (keys from the top), as JSON; `None` if it isn't there.
pub(crate) fn get(text: &str, path: &[&str]) -> Result<Option<Value>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let root = parse(text)?;
    let mut node = &root;
    for key in path {
        let Some(o) = &node.object else { return Ok(None) };
        match o.members.iter().rev().find(|m| m.key == *key) {
            Some(m) => node = &m.value,
            None => return Ok(None),
        }
    }
    serde_json::from_str(&plain(text, node.start, node.end)).map(Some).map_err(|e| format!("reading it back: {e}"))
}

/// The whitespace a line starts with, for the line `at` is on.
fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let rest = &text[line..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// One level of indentation, as the file has it (two spaces when it says nothing).
fn indent_unit(text: &str, root: &Node) -> String {
    let o = root.object.as_ref().unwrap();
    o.members.first().map(|m| indent_at(text, m.start).to_string()).filter(|s| !s.is_empty() && text[..o.members[0].start].contains('\n')).unwrap_or_else(|| "  ".into())
}

/// `value` as pretty JSON, its lines after the first indented by `base`.
fn render(value: &Value, unit: &str, base: &str) -> String {
    let mut out = String::new();
    pretty(value, unit, base, &mut out);
    out
}

/// The keys a server entry reads best in, first; the rest follow in order.
const FIRST: &[&str] = &["type", "command", "args", "env"];

fn pretty(value: &Value, unit: &str, indent: &str, out: &mut String) {
    let inner = format!("{indent}{unit}");
    match value {
        Value::Object(o) if !o.is_empty() => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by_key(|k| FIRST.iter().position(|f| f == k).unwrap_or(FIRST.len()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                out.push_str(&inner);
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push_str(": ");
                pretty(&o[*k], unit, &inner, out);
            }
            out.push('\n');
            out.push_str(indent);
            out.push('}');
        }
        Value::Array(a) if !a.is_empty() => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                out.push_str(&inner);
                pretty(v, unit, &inner, out);
            }
            out.push('\n');
            out.push_str(indent);
            out.push(']');
        }
        v => out.push_str(&v.to_string()),
    }
}

/// Whether `s` holds only whitespace and comments.
fn blank(s: &str) -> bool {
    let mut p = Parser { text: s.as_bytes(), at: 0 };
    p.skip().is_ok() && p.at == s.len()
}

/// Set the member at `path` to `value`, making the objects on the way, and change nothing else.
pub(crate) fn set(text: &str, path: &[&str], value: &Value) -> Result<String, String> {
    assert!(!path.is_empty());
    let text = if text.trim().is_empty() { "{}\n" } else { text };
    let root = parse(text)?;
    let unit = indent_unit(text, &root);
    let mut node = &root;
    let mut depth = 0;
    loop {
        let o = node.object.as_ref().ok_or_else(|| format!("\"{}\" isn't an object", path[..depth].join(".")))?;
        let key = path[depth];
        if let Some(m) = o.members.iter().rev().find(|m| m.key == key) {
            if depth + 1 == path.len() {
                let base = indent_at(text, m.start);
                let out = format!("{}{}{}", &text[..m.value.start], render(value, &unit, base), &text[m.value.end..]);
                return check(out, path, value);
            }
            node = &m.value;
            depth += 1;
            continue;
        }
        // Not there: a new member, holding the rest of the path.
        let mut nested = value.clone();
        for k in path[depth + 1..].iter().rev() {
            nested = Value::Object([(k.to_string(), nested)].into_iter().collect());
        }
        let outer = if depth == 0 { String::new() } else { indent_at(text, node.start).to_string() };
        let inner = match o.members.first() {
            Some(m) if text[o.open..m.start].contains('\n') => indent_at(text, m.start).to_string(),
            _ => format!("{outer}{unit}"),
        };
        let member = format!("{}: {}", serde_json::to_string(key).unwrap(), render(&nested, &unit, &inner));
        let out = match o.members.last() {
            None => {
                let close_indent = if depth == 0 { "" } else { outer.as_str() };
                format!("{}\n{inner}{member}\n{close_indent}{}", &text[..o.open], &text[o.close..])
            }
            Some(last) => {
                // After the last member's line (its comma, and a comment ending the line, stay
                // with it), with a comma after it if it had none.
                let after = last.comma.map_or(last.value.end, |c| c + 1);
                let rest = &text[after..o.close];
                let line_end = match rest.find('\n') {
                    Some(i) if blank(&rest[..i]) => after + i,
                    _ => after,
                };
                let comma = if last.comma.is_some() { "" } else { "," };
                if line_end == after && !text[o.open..o.close].contains('\n') {
                    // All on one line: keep it on one line.
                    format!("{}{comma} {}{}", &text[..after], serde_json::to_string(key).unwrap() + ": " + &serde_json::to_string(&nested).unwrap(), &text[after..])
                } else {
                    format!("{}{comma}{}\n{inner}{member}{}", &text[..last.value.end], &text[last.value.end..line_end], &text[line_end..])
                }
            }
        };
        return check(out, path, value);
    }
}

/// Remove the member at `path`; `None` if it wasn't there.
pub(crate) fn remove(text: &str, path: &[&str]) -> Result<Option<String>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let root = parse(text)?;
    let mut node = &root;
    for (depth, key) in path.iter().enumerate() {
        let Some(o) = &node.object else { return Ok(None) };
        let Some(i) = o.members.iter().rposition(|m| m.key == *key) else { return Ok(None) };
        let m = &o.members[i];
        if depth + 1 < path.len() {
            node = &m.value;
            continue;
        }
        // Its line goes with it when nothing else is on it.
        let line_start = text[..m.start].rfind('\n').map_or(0, |n| n + 1);
        let own_line = text[line_start..m.start].trim().is_empty();
        let out = if o.members.len() == 1 {
            // The only member: the object becomes `{}`.
            format!("{}{}", &text[..o.open], &text[o.close..])
        } else if let Some(c) = m.comma {
            // Through its comma.
            let mut to = c + 1;
            let rest = &text[to..];
            if let Some(n) = rest.find('\n').filter(|&n| own_line && rest[..n].trim().is_empty()) {
                to += n + 1;
            }
            format!("{}{}", &text[..if own_line { line_start } else { m.start }], &text[to..])
        } else {
            // The last: the comma before it goes (a comment after that comma stays).
            let comma = o.members[i - 1].comma.expect("a member before another has a comma");
            let from = if own_line && line_start > 0 { line_start - 1 } else { m.start };
            format!("{}{}{}", &text[..comma], &text[comma + 1..from], &text[m.value.end..])
        };
        parse(&out).map_err(|e| format!("removing it would break the file ({e})"))?;
        return Ok(Some(out));
    }
    Ok(None)
}

/// The edited text, checked: it still parses, and holds `value` at `path`.
fn check(out: String, path: &[&str], value: &Value) -> Result<String, String> {
    match get(&out, path)? {
        Some(v) if &v == value => Ok(out),
        _ => Err("the edit didn't read back as written; the file is left as it was".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> Value {
        json!({ "command": "holon", "args": ["mcp"] })
    }

    #[test]
    fn a_new_member_goes_after_the_others_and_nothing_else_moves() {
        let text = "{\n    \"z\": 1,\n    \"a\": { \"b\": [1, 2] } // keep me\n}\n";
        let out = set(text, &["mcpServers", "holon"], &entry()).unwrap();
        assert_eq!(
            out,
            "{\n    \"z\": 1,\n    \"a\": { \"b\": [1, 2] }, // keep me\n    \"mcpServers\": {\n        \"holon\": {\n            \"command\": \"holon\",\n            \"args\": [\n                \"mcp\"\n            ]\n        }\n    }\n}\n"
        );
    }

    #[test]
    fn into_an_object_that_is_there_with_comments_and_a_trailing_comma() {
        let text = "// VS Code\n{\n  \"servers\": {\n    \"other\": { \"command\": \"x\" }, /* theirs */\n  },\n}\n";
        let out = set(text, &["servers", "holon"], &entry()).unwrap();
        assert!(out.starts_with("// VS Code\n{\n  \"servers\": {\n    \"other\": { \"command\": \"x\" }, /* theirs */\n    \"holon\": {\n      \"command\": \"holon\","), "{out}");
        assert!(out.ends_with("    }\n  },\n}\n"), "{out}");
        assert_eq!(get(&out, &["servers", "other", "command"]).unwrap(), Some(json!("x")));
    }

    #[test]
    fn a_member_that_is_there_is_replaced_in_place() {
        let text = "{\n  \"mcpServers\": {\n    \"holon\": { \"command\": \"old\" },\n    \"next\": {}\n  }\n}";
        let out = set(text, &["mcpServers", "holon"], &json!({ "command": "new" })).unwrap();
        assert_eq!(out, "{\n  \"mcpServers\": {\n    \"holon\": {\n      \"command\": \"new\"\n    },\n    \"next\": {}\n  }\n}");
    }

    #[test]
    fn an_empty_or_missing_file_and_an_empty_object() {
        assert_eq!(set("", &["mcpServers", "holon"], &json!({})).unwrap(), "{\n  \"mcpServers\": {\n    \"holon\": {}\n  }\n}\n");
        assert_eq!(set("{}", &["a"], &json!(1)).unwrap(), "{\n  \"a\": 1\n}");
        assert_eq!(set("{ \"a\": 1 }", &["b"], &json!(2)).unwrap(), "{ \"a\": 1, \"b\": 2 }");
    }

    #[test]
    fn removed_with_its_comma_and_the_rest_kept() {
        let text = "{\n  \"mcpServers\": {\n    \"a\": 1,\n    \"holon\": { \"command\": \"x\" },\n    \"b\": 2\n  }\n}\n";
        assert_eq!(remove(text, &["mcpServers", "holon"]).unwrap().unwrap(), "{\n  \"mcpServers\": {\n    \"a\": 1,\n    \"b\": 2\n  }\n}\n");
        let last = "{\n  \"s\": {\n    \"a\": 1,\n    \"holon\": 2\n  }\n}";
        assert_eq!(remove(last, &["s", "holon"]).unwrap().unwrap(), "{\n  \"s\": {\n    \"a\": 1\n  }\n}");
        let only = "{\n  \"s\": {\n    \"holon\": 2\n  }\n}";
        assert_eq!(remove(only, &["s", "holon"]).unwrap().unwrap(), "{\n  \"s\": {}\n}");
        assert_eq!(remove(only, &["s", "nobody"]).unwrap(), None);
    }

    #[test]
    fn what_isnt_json_is_refused_and_left_alone() {
        assert!(set("{ \"a\": }", &["b"], &json!(1)).unwrap_err().contains("line 1"));
        assert!(set("[1]", &["b"], &json!(1)).unwrap_err().contains("isn't a JSON object"));
        assert!(set("{ \"a\": 1 }", &["a", "b"], &json!(1)).unwrap_err().contains("isn't an object"));
    }
}
