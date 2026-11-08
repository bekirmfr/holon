//! CSV both ways (RFC 4180; host-capabilities L4 §5.1.1): a payment file's rows read into records
//! or lists of texts, and records or lists written back, quoted where they must be. A file that is
//! malformed is refused, not guessed at.

use crate::numbers::js_string;
use crate::value::Value;
use std::collections::BTreeMap;

/// `header`'s values, its family: the first row names the columns, or every row is data.
pub(crate) const HEADERS: &[&str] = &["first_row", "none"];

/// `separator`'s values, its family.
pub(crate) const SEPARATORS: &[&str] = &[",", ";", "\t", "|"];

/// The separator a family value names.
fn separator(name: &str) -> Option<char> {
    SEPARATORS.contains(&name).then(|| name.chars().next()).flatten()
}

/// A text's rows, each its fields: CRLF or LF between them, a quoted field with `""` for a quote
/// and the separator or a line break inside it, a leading byte-order mark and empty lines dropped.
/// Nothing for a quote that never closes, text after a closing quote, or a quote inside a field
/// that isn't quoted.
fn rows(text: &str, sep: char) -> Option<Vec<Vec<String>>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    // Whether the row so far has anything in it (a quoted empty field does), so an empty line
    // isn't a row of one empty field.
    let mut begun = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if field.is_empty() => {
                begun = true;
                loop {
                    match chars.next()? {
                        '"' if chars.peek() == Some(&'"') => {
                            chars.next();
                            field.push('"');
                        }
                        '"' => break,
                        c => field.push(c),
                    }
                }
                // Only a separator, a line break or the end may follow a closing quote.
                match chars.peek() {
                    None | Some('\n') | Some('\r') => {}
                    Some(&c) if c == sep => {}
                    _ => return None,
                }
            }
            '"' => return None,
            c if c == sep => {
                begun = true;
                row.push(std::mem::take(&mut field));
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                if begun {
                    row.push(std::mem::take(&mut field));
                    out.push(std::mem::take(&mut row));
                }
                begun = false;
            }
            c => {
                begun = true;
                field.push(c);
            }
        }
    }
    if begun {
        row.push(field);
        out.push(row);
    }
    Some(out)
}

/// `csv_parse`: with `header: "first_row"`, a list of records keyed by the first row's names (none
/// twice, and every row as many fields); with `"none"`, a list of lists. Every field is a text.
pub(crate) fn parse(text: &str, header: &str, sep: &str) -> Option<Value> {
    let rows = rows(text, separator(sep)?)?;
    let text_list = |row: Vec<String>| Value::List(row.into_iter().map(Value::Text).collect());
    match header {
        "none" => Some(Value::List(rows.into_iter().map(text_list).collect())),
        "first_row" => {
            let mut rows = rows.into_iter();
            let Some(names) = rows.next() else { return Some(Value::List(vec![])) };
            let mut seen = std::collections::BTreeSet::new();
            if !names.iter().all(|n| seen.insert(n)) {
                return None;
            }
            let mut out = Vec::new();
            for row in rows {
                if row.len() != names.len() {
                    return None;
                }
                let record: BTreeMap<String, Value> = names.iter().cloned().zip(row.into_iter().map(Value::Text)).collect();
                out.push(Value::Record(record));
            }
            Some(Value::List(out))
        }
        _ => None,
    }
}

/// A field as it is written: a text as it is, a number as `String(n)`, a Boolean as `true` or
/// `false`; nothing for anything else.
fn field_text(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Number(_) | Value::Boolean(_) => Some(js_string(v)),
        _ => None,
    }
}

/// A line of fields, each quoted if it holds the separator, a quote or a line break, and a lone
/// empty field quoted so the line isn't read as an empty one.
fn line(out: &mut String, fields: &[String], sep: char) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(sep);
        }
        if f.contains([sep, '"', '\r', '\n']) || (f.is_empty() && fields.len() == 1) {
            out.push('"');
            out.push_str(&f.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(f);
        }
    }
    out.push_str("\r\n");
}

/// `csv_format`: given `columns`, each row a record whose fields are written in that order (a
/// missing one empty), the names first unless `header: "none"`; without, each row a list. Each
/// line ends with CRLF, the last too.
pub(crate) fn format(rows: &[Value], columns: Option<&Value>, header: &str, sep: &str) -> Option<String> {
    let sep = separator(sep)?;
    if !HEADERS.contains(&header) {
        return None;
    }
    let mut out = String::new();
    match columns {
        Some(Value::List(names)) => {
            let names: Vec<String> = names.iter().map(|n| if let Value::Text(s) = n { Some(s.clone()) } else { None }).collect::<Option<_>>()?;
            if header == "first_row" {
                line(&mut out, &names, sep);
            }
            for row in rows {
                let Value::Record(r) = row else { return None };
                let fields: Vec<String> = names.iter().map(|n| r.get(n).map_or(Some(String::new()), field_text)).collect::<Option<_>>()?;
                line(&mut out, &fields, sep);
            }
        }
        Some(_) => return None,
        None => {
            for row in rows {
                let Value::List(items) = row else { return None };
                let fields: Vec<String> = items.iter().map(field_text).collect::<Option<_>>()?;
                line(&mut out, &fields, sep);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn rec(pairs: &[(&str, &str)]) -> Value {
        Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), t(v))).collect())
    }
    fn texts(rows: &[&[&str]]) -> Value {
        Value::List(rows.iter().map(|r| Value::List(r.iter().map(|s| t(s)).collect())).collect())
    }

    #[test]
    fn reads_rfc_4180() {
        let file = "\u{feff}id,amount,memo\r\n1,10.50,\"rent, May\"\r\n\r\n2,0.20,\"say \"\"hi\"\"\nthen go\"\n3,1,\n";
        assert_eq!(
            parse(file, "first_row", ","),
            Some(Value::List(vec![
                rec(&[("id", "1"), ("amount", "10.50"), ("memo", "rent, May")]),
                rec(&[("id", "2"), ("amount", "0.20"), ("memo", "say \"hi\"\nthen go")]),
                rec(&[("id", "3"), ("amount", "1"), ("memo", "")]),
            ]))
        );
        assert_eq!(parse("a;b\n1;2", "none", ";"), Some(texts(&[&["a", "b"], &["1", "2"]])), "no line break at the end");
        assert_eq!(parse("a\tb\n", "none", "\t"), Some(texts(&[&["a", "b"]])));
        assert_eq!(parse("\"\"\n", "none", ","), Some(texts(&[&[""]])), "a quoted empty field alone is a row");
        assert_eq!(parse(",\n", "none", ","), Some(texts(&[&["", ""]])));
        assert_eq!(parse("", "first_row", ","), Some(Value::List(vec![])));
        assert_eq!(parse("id,amount\n", "first_row", ","), Some(Value::List(vec![])));
    }

    #[test]
    fn refuses_a_malformed_file() {
        for (file, why) in [
            ("a,\"b\n", "a quote that never closes"),
            ("a,\"b\"c\n", "text after a closing quote"),
            ("a,b\"c\n", "a quote inside an unquoted field"),
            ("a,b\n1\n", "a row short of the header"),
            ("a,b\n1,2,3\n", "a row past it"),
            ("a,a\n1,2\n", "a column named twice"),
        ] {
            assert_eq!(parse(file, "first_row", ","), None, "{why}");
        }
        assert_eq!(parse("a", "first_row", ":"), None, "not a separator");
    }

    #[test]
    fn writes_records_in_the_columns_order_and_quotes_what_it_must() {
        let rows = [
            Value::Record([("amount".into(), Value::Number(10.5)), ("id".into(), t("1")), ("memo".into(), t("rent, May"))].into()),
            Value::Record([("id".into(), t("2")), ("memo".into(), t("say \"hi\"\nthen go")), ("ok".into(), Value::Boolean(true))].into()),
        ];
        let cols = Value::List(vec![t("id"), t("amount"), t("memo")]);
        let text = format(&rows, Some(&cols), "first_row", ",").unwrap();
        assert_eq!(text, "id,amount,memo\r\n1,10.5,\"rent, May\"\r\n2,,\"say \"\"hi\"\"\nthen go\"\r\n");
        assert_eq!(format(&rows, Some(&cols), "none", ";").unwrap(), "1;10.5;rent, May\r\n2;;\"say \"\"hi\"\"\nthen go\"\r\n");
        // It reads back as it was written.
        assert_eq!(
            parse(&text, "first_row", ","),
            Some(Value::List(vec![
                rec(&[("id", "1"), ("amount", "10.5"), ("memo", "rent, May")]),
                rec(&[("id", "2"), ("amount", ""), ("memo", "say \"hi\"\nthen go")]),
            ]))
        );
        assert_eq!(format(&[Value::List(vec![t("a"), Value::Number(1.0)]), Value::List(vec![t("")])], None, "first_row", ",").unwrap(), "a,1\r\n\"\"\r\n");
        assert_eq!(format(&[], None, "first_row", ",").unwrap(), "");
        assert_eq!(format(&[Value::List(vec![Value::List(vec![])])], None, "first_row", ","), None, "a list isn't a field");
        assert_eq!(format(&[rec(&[("a", "1")])], None, "first_row", ","), None, "records need their columns");
        assert_eq!(format(&[Value::List(vec![])], Some(&cols), "first_row", ","), None, "and columns need records");
    }
}
