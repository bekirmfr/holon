//! XML both ways (host-capabilities L4 §5.1.5): an element is a list, as JsonML writes one, its
//! name, then a record of its attributes when it has any, then its children in order, each an
//! element or a text. Read from XML 1.0 without a DTD, written back, and read through paths as
//! Python's ElementTree reads one. A DOCTYPE is refused, so no entity is ever defined, expanded or
//! fetched.

use crate::numbers::js_string;
use crate::value::Value;
use std::collections::BTreeMap;

/// `declaration`'s values, its family: the XML declaration first, or nothing.
pub(crate) const DECLARATIONS: &[&str] = &["utf8", "none"];

/// `layout`'s values, its family: on one line, or indented two spaces a level.
pub(crate) const LAYOUTS: &[&str] = &["compact", "indented"];

/// How deep elements nest, at most, read or written: `json_parse`'s bound.
const MAX_DEPTH: usize = 64;

/// XML's NameStartChar (XML 1.0, fifth edition, §2.3).
fn name_start(c: char) -> bool {
    matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}'
        | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}'
        | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}' | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}'
        | '\u{10000}'..='\u{effff}')
}

/// XML's NameChar.
fn name_char(c: char) -> bool {
    name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
}

/// Whether `s` is an XML name.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(name_start) && chars.all(name_char)
}

/// XML's Char: what a document may hold at all.
fn is_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

/// XML's white space.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// An element's name, its attributes when it has any, and its children.
type Parts<'v> = (&'v str, Option<&'v BTreeMap<String, Value>>, &'v [Value]);

/// An element's parts, or nothing for a value that isn't one.
fn parts(element: &Value) -> Option<Parts<'_>> {
    let Value::List(items) = element else { return None };
    let Some((Value::Text(name), rest)) = items.split_first() else { return None };
    Some(match rest.split_first() {
        Some((Value::Record(attributes), children)) => (name.as_str(), Some(attributes), children),
        _ => (name.as_str(), None, rest),
    })
}

/// A text, a number or a Boolean as XML writes it; nothing for anything else.
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Number(_) | Value::Boolean(_) => Some(js_string(v)),
        _ => None,
    }
}

/// A text with the five named entities and character references read; nothing for any other
/// entity, or a reference to a character XML can't hold.
fn unescape(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.find(';')?;
        out.push(match &after[..end] {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            entity => {
                let (digits, radix) = match entity.strip_prefix("#x") {
                    Some(hex) => (hex, 16),
                    None => (entity.strip_prefix('#')?, 10),
                };
                if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                    return None;
                }
                char::from_u32(u32::from_str_radix(digits, radix).ok()?).filter(|&c| is_char(c))?
            }
        });
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// A place in the text being read.
struct Reader<'a> {
    text: &'a str,
    at: usize,
    /// The values made so far: each element's list and name, each attribute, each text.
    made: usize,
}

impl<'a> Reader<'a> {
    /// Counts `more` values made: nothing past [`MAX_ITEMS`](crate::MAX_ITEMS).
    fn made(&mut self, more: usize) -> Option<()> {
        self.made += more;
        (self.made <= crate::MAX_ITEMS).then_some(())
    }

    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    /// Steps past `prefix` when the text goes on with it.
    fn eat(&mut self, prefix: &str) -> bool {
        let found = self.rest().starts_with(prefix);
        if found {
            self.at += prefix.len();
        }
        found
    }

    /// Steps past white space; whether there was any.
    fn space(&mut self) -> bool {
        let rest = self.rest();
        let n = rest.len() - rest.trim_start_matches(is_space).len();
        self.at += n;
        n > 0
    }

    /// The text up to `end`, stepping past both.
    fn until(&mut self, end: &str) -> Option<&'a str> {
        let rest = self.rest();
        let i = rest.find(end)?;
        self.at += i + end.len();
        Some(&rest[..i])
    }

    fn name(&mut self) -> Option<String> {
        let rest = self.rest();
        let end = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
        is_name(&rest[..end]).then(|| {
            self.at += end;
            rest[..end].to_string()
        })
    }

    /// A comment's rest, after `<!--`: no `--` inside, nor a `-` before its end.
    fn comment(&mut self) -> Option<()> {
        let body = self.until("-->")?;
        (!body.contains("--") && !body.ends_with('-')).then_some(())
    }

    /// A processing instruction, from `<?`: its target a name other than `xml`.
    fn instruction(&mut self) -> Option<()> {
        self.at += 2;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return None;
        }
        if !self.space() && !self.rest().starts_with("?>") {
            return None;
        }
        self.until("?>").map(|_| ())
    }

    /// White space, comments and processing instructions, as may come around the root.
    fn misc(&mut self) -> Option<()> {
        loop {
            self.space();
            if self.eat("<!--") {
                self.comment()?;
            } else if self.rest().starts_with("<?") {
                self.instruction()?;
            } else {
                return Some(());
            }
        }
    }

    /// An attribute's value, after its quote: no `<`, white space read as a space, references
    /// read.
    fn attribute_value(&mut self, quote: &str) -> Option<String> {
        let raw = self.until(quote)?;
        if raw.contains('<') {
            return None;
        }
        unescape(&raw.replace(is_space, " "))
    }

    /// An element, from its `<`, `depth` elements inside the root.
    fn element(&mut self, depth: usize) -> Option<Value> {
        self.made(2)?;
        if depth >= MAX_DEPTH || !self.eat("<") {
            return None;
        }
        let name = self.name()?;
        let mut attributes = BTreeMap::new();
        loop {
            let spaced = self.space();
            if self.eat("/>") {
                return Some(element(name, attributes, Vec::new()));
            }
            if self.eat(">") {
                break;
            }
            if !spaced {
                return None;
            }
            let key = self.name()?;
            self.space();
            if !self.eat("=") {
                return None;
            }
            self.space();
            let quote = if self.eat("\"") { "\"" } else if self.eat("'") { "'" } else { return None };
            let value = self.attribute_value(quote)?;
            self.made(1)?;
            if attributes.insert(key, Value::Text(value)).is_some() {
                return None;
            }
        }
        let mut children = Vec::new();
        // The text since the last child element: character data, CDATA and the text around a
        // comment, as one.
        let mut text = String::new();
        loop {
            if self.eat("</") {
                let end = self.name()?;
                self.space();
                if end != name || !self.eat(">") {
                    return None;
                }
                let kept = flush(&mut text, &mut children);
                self.made(kept)?;
                return Some(element(name, attributes, children));
            } else if self.eat("<!--") {
                self.comment()?;
            } else if self.eat("<![CDATA[") {
                text.push_str(self.until("]]>")?);
            } else if self.rest().starts_with("<?") {
                self.instruction()?;
            } else if self.rest().starts_with('<') {
                let kept = flush(&mut text, &mut children);
                self.made(kept)?;
                children.push(self.element(depth + 1)?);
            } else if self.rest().is_empty() {
                return None;
            } else {
                let rest = self.rest();
                let end = rest.find('<').unwrap_or(rest.len());
                if rest[..end].contains("]]>") {
                    return None;
                }
                text.push_str(&unescape(&rest[..end])?);
                self.at += end;
            }
        }
    }
}

/// An element's list: its name, its attributes when it has any, its children.
fn element(name: String, attributes: BTreeMap<String, Value>, children: Vec<Value>) -> Value {
    let mut items = vec![Value::Text(name)];
    if !attributes.is_empty() {
        items.push(Value::Record(attributes));
    }
    items.extend(children);
    Value::List(items)
}

/// Ends a text child: kept unless it is only white space, the indentation between elements. How
/// many it kept, none or one.
fn flush(text: &mut String, children: &mut Vec<Value>) -> usize {
    let text = std::mem::take(text);
    if text.chars().all(is_space) {
        return 0;
    }
    children.push(Value::Text(text));
    1
}

/// `xml_parse`: the root element of an XML 1.0 document, or nothing for text that isn't well-formed
/// XML, or that has a DOCTYPE.
pub(crate) fn parse(text: &str) -> Option<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n").replace('\r', "\n");
    if !text.chars().all(is_char) {
        return None;
    }
    let mut reader = Reader { text: &text, at: 0, made: 0 };
    let declared = reader.rest().strip_prefix("<?xml").is_some_and(|rest| rest.starts_with(is_space));
    if declared && !reader.until("?>")?[5..].trim_start_matches(is_space).starts_with("version") {
        return None;
    }
    reader.misc()?;
    let root = reader.element(0)?;
    reader.misc()?;
    reader.rest().is_empty().then_some(root)
}

/// Writes `s` escaped, as a text or an attribute's value: nothing for a character XML can't hold.
fn escape(s: &str, attribute: bool, out: &mut String) -> Option<()> {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            '"' if attribute => out.push_str("&quot;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            c if is_char(c) => out.push(c),
            _ => return None,
        }
    }
    Some(())
}

/// Writes an element, `depth` inside the root; `indent` its level when it is laid out indented.
fn write(element: &Value, indent: Option<usize>, depth: usize, out: &mut String) -> Option<()> {
    let (name, attributes, children) = parts(element)?;
    if depth >= MAX_DEPTH || !is_name(name) {
        return None;
    }
    out.push('<');
    out.push_str(name);
    for (key, value) in attributes.into_iter().flatten() {
        if !is_name(key) {
            return None;
        }
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        escape(&scalar(value)?, true, out)?;
        out.push('"');
    }
    if children.is_empty() {
        out.push_str("/>");
        return Some(());
    }
    out.push('>');
    // Laid out indented only when every child is an element: text keeps its spaces as written.
    let block = indent.filter(|_| children.iter().all(|c| matches!(c, Value::List(_))));
    for child in children {
        if let Value::List(_) = child {
            if let Some(level) = block {
                out.push('\n');
                out.push_str(&"  ".repeat(level + 1));
            }
            write(child, block.map(|level| level + 1), depth + 1, out)?;
        } else {
            escape(&scalar(child)?, false, out)?;
        }
    }
    if let Some(level) = block {
        out.push('\n');
        out.push_str(&"  ".repeat(level));
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
    Some(())
}

/// `xml_format`: an element's text, the XML declaration first unless `declaration` is `none`.
/// Nothing for a name that isn't an XML name, a character XML can't hold, an item that is neither
/// an element, a text, a number nor a Boolean (but the attributes' record, second), or elements
/// nested past 64.
pub(crate) fn format(element: &Value, declaration: &str, layout: &str) -> Option<String> {
    let indent = match layout {
        "compact" => None,
        "indented" => Some(0),
        _ => return None,
    };
    let mut out = match declaration {
        "utf8" => "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_string(),
        "none" => String::new(),
        _ => return None,
    };
    write(element, indent, 0, &mut out)?;
    Some(out)
}

/// The elements along `path`, a step at a time from `element`'s children, each step a name or
/// `*`.
fn along<'v>(element: &'v Value, path: &str) -> Option<Vec<&'v Value>> {
    parts(element)?;
    let mut found = vec![element];
    for step in path.split('/') {
        if step != "*" && !is_name(step) {
            return None;
        }
        found = found
            .into_iter()
            .flat_map(|e| parts(e).map_or(&[][..], |(_, _, children)| children))
            .filter(|c| parts(c).is_some_and(|(name, _, _)| step == "*" || name == step))
            .collect();
    }
    Some(found)
}

/// `xml_find`: the elements along `path`, in document order; an empty list when none matches.
pub(crate) fn find(element: &Value, path: &str) -> Option<Value> {
    Some(Value::List(along(element, path)?.into_iter().cloned().collect()))
}

/// Every text in `element`, at any depth, joined.
fn text_of(element: &Value, out: &mut String) -> Option<()> {
    for child in parts(element)?.2 {
        match child {
            Value::List(_) => text_of(child, out)?,
            _ => out.push_str(&scalar(child)?),
        }
    }
    Some(())
}

/// `xml_text`: `element`'s text, or with `path` its first match's; nothing when none matches.
pub(crate) fn text(element: &Value, path: Option<&str>) -> Option<Value> {
    let target = match path {
        Some(path) => along(element, path)?.into_iter().next()?,
        None => element,
    };
    let mut out = String::new();
    text_of(target, &mut out)?;
    Some(Value::Text(out))
}

/// `xml_attribute`: the attribute's value, or nothing.
pub(crate) fn attribute(element: &Value, name: &str) -> Option<Value> {
    parts(element)?.1?.get(name).and_then(scalar).map(Value::Text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn el(items: Vec<Value>) -> Value {
        Value::List(items)
    }

    fn attrs(pairs: &[(&str, &str)]) -> Value {
        Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), txt(v))).collect())
    }

    #[test]
    fn a_document_reads_into_lists_and_writes_back() {
        let file = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Document xmlns=\"urn:iso:std:iso:20022:tech:xsd:pain.001.001.09\"><GrpHdr><MsgId>M1</MsgId><NbOfTxs>2</NbOfTxs></GrpHdr><Amt><InstdAmt Ccy=\"EUR\">100.00</InstdAmt></Amt><Empty/></Document>";
        let doc = parse(file).unwrap();
        let expected = el(vec![
            txt("Document"),
            attrs(&[("xmlns", "urn:iso:std:iso:20022:tech:xsd:pain.001.001.09")]),
            el(vec![txt("GrpHdr"), el(vec![txt("MsgId"), txt("M1")]), el(vec![txt("NbOfTxs"), txt("2")])]),
            el(vec![txt("Amt"), el(vec![txt("InstdAmt"), attrs(&[("Ccy", "EUR")]), txt("100.00")])]),
            el(vec![txt("Empty")]),
        ]);
        assert_eq!(doc, expected);
        assert_eq!(format(&doc, "utf8", "compact").as_deref(), Some(file), "the same file, written back");
        let indented = "<Document xmlns=\"urn:iso:std:iso:20022:tech:xsd:pain.001.001.09\">\n  <GrpHdr>\n    <MsgId>M1</MsgId>\n    <NbOfTxs>2</NbOfTxs>\n  </GrpHdr>\n  <Amt>\n    <InstdAmt Ccy=\"EUR\">100.00</InstdAmt>\n  </Amt>\n  <Empty/>\n</Document>";
        assert_eq!(format(&doc, "none", "indented").as_deref(), Some(indented));
        assert_eq!(parse(indented), Some(doc), "the indentation is read as nothing");
    }

    #[test]
    fn text_is_read_as_written_and_escaped_back() {
        let doc = parse("<a t='x\ty &amp; &#34;q&#34;'>&lt;b&gt; &amp; <![CDATA[<raw> & ]]><!-- note --> &#x20AC;5 <i>it</i> after</a>").unwrap();
        assert_eq!(
            doc,
            el(vec![
                txt("a"),
                attrs(&[("t", "x y & \"q\"")]),
                txt("<b> & <raw> &  €5 "),
                el(vec![txt("i"), txt("it")]),
                txt(" after"),
            ])
        );
        // Mixed content stays on its line, indented or not.
        assert_eq!(
            format(&doc, "none", "indented").as_deref(),
            Some("<a t=\"x y &amp; &quot;q&quot;\">&lt;b&gt; &amp; &lt;raw&gt; &amp;  €5 <i>it</i> after</a>")
        );
        let tabbed = el(vec![txt("a"), attrs(&[("t", "1\t2\n3\r")]), txt("x\ry")]);
        let written = format(&tabbed, "none", "compact").unwrap();
        assert_eq!(written, "<a t=\"1&#9;2&#10;3&#13;\">x&#13;y</a>");
        assert_eq!(parse(&written), Some(tabbed), "tabs, line breaks and carriage returns read back");
        assert_eq!(parse("<a>1\r\n2\r3</a>"), Some(el(vec![txt("a"), txt("1\n2\n3")])), "line ends made \\n");
    }

    #[test]
    fn numbers_and_booleans_are_written_as_javascript_writes_them() {
        let built = el(vec![txt("n"), Value::Record(BTreeMap::from([("on".to_string(), Value::Boolean(true))])), Value::Number(3.0), txt(" "), Value::Number(0.1)]);
        assert_eq!(format(&built, "none", "compact").as_deref(), Some("<n on=\"true\">3 0.1</n>"));
    }

    #[test]
    fn what_isnt_well_formed_or_has_a_doctype_is_nothing() {
        let bad = [
            "<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>",
            "<!DOCTYPE a SYSTEM \"http://example.com/a.dtd\"><a/>",
            "<a>&x;</a>",
            "<a>&#0;</a>",
            "<a>&#xD800;</a>",
            "<a>&amp</a>",
            "<a><b></a></b>",
            "<a>",
            "<a></b>",
            "<a x='1' x='2'/>",
            "<a x=1/>",
            "<a x='<'/>",
            "<1a/>",
            "<a>]]></a>",
            "<a/><b/>",
            "text<a/>",
            "<a/>text",
            "<a><!-- a -- b --></a>",
            "<a><?xml version='1.0'?></a>",
            "<a>\u{1}</a>",
            "",
            "<?xml encoding='UTF-8'?><a/>",
        ];
        for text in bad {
            assert_eq!(parse(text), None, "{text:?}");
        }
        let deep = format!("{}{}", "<a>".repeat(65), "</a>".repeat(65));
        assert_eq!(parse(&deep), None, "elements nest at most 64 deep");
        let ok = format!("{}{}", "<a>".repeat(64), "</a>".repeat(64));
        assert!(parse(&ok).is_some());
        assert!(parse("\u{feff}<?xml version='1.0'?>\n<!-- before --><?pi data?><a/>\n<!-- after -->\n").is_some(), "a BOM, the declaration, comments and instructions around the root");
    }

    #[test]
    fn what_xml_cant_hold_is_not_written() {
        for bad in [
            el(vec![txt("a b")]),
            el(vec![txt("a"), txt("\u{1}")]),
            el(vec![txt("a"), attrs(&[("1x", "v")])]),
            el(vec![txt("a"), txt("x"), attrs(&[("late", "v")])]),
            el(vec![txt("a"), Value::Bytes(vec![1])]),
            el(vec![Value::Number(1.0)]),
            el(vec![]),
            txt("a"),
        ] {
            assert_eq!(format(&bad, "utf8", "compact"), None, "{bad:?}");
        }
        let mut deep = el(vec![txt("a")]);
        for _ in 0..64 {
            deep = el(vec![txt("a"), deep]);
        }
        assert_eq!(format(&deep, "none", "compact"), None, "nested past 64");
    }

    #[test]
    fn paths_find_elements_their_text_and_attributes() {
        let report = parse(
            "<Document><Rpt><Pmt><Tx><Id>P1</Id><Sts>ACCP</Sts></Tx><Tx><Id>P2</Id><Sts>RJCT</Sts><Rsn><Cd>AC04</Cd></Rsn></Tx></Pmt><Pmt><Tx><Id>P3</Id><Sts>ACCP</Sts></Tx></Pmt></Rpt><Amt Ccy='EUR'>1.<b>5</b>0</Amt></Document>",
        )
        .unwrap();
        let Some(Value::List(txs)) = find(&report, "Rpt/Pmt/Tx") else { panic!() };
        let ids: Vec<_> = txs.iter().map(|t| text(t, Some("Id"))).collect();
        assert_eq!(ids, [Some(txt("P1")), Some(txt("P2")), Some(txt("P3"))], "in document order, across parents");
        assert_eq!(text(&txs[1], Some("Rsn/Cd")), Some(txt("AC04")));
        assert_eq!(text(&txs[0], Some("Rsn/Cd")), None, "nothing when none matches");
        assert_eq!(find(&report, "Rpt/*/Tx/Sts").map(|v| matches!(v, Value::List(l) if l.len() == 3)), Some(true), "* is any name");
        assert_eq!(find(&report, "Nothing"), Some(Value::List(vec![])));
        assert_eq!(find(&report, "Rpt//Tx"), None, "an empty step isn't one");
        assert_eq!(text(&report, Some("Amt")), Some(txt("1.50")), "every text inside, joined");
        let amount = along(&report, "Amt").unwrap()[0];
        assert_eq!(attribute(amount, "Ccy"), Some(txt("EUR")));
        assert_eq!(attribute(amount, "Missing"), None);
        assert_eq!(attribute(&report, "Ccy"), None, "an element without attributes");
        assert_eq!(text(&txt("not an element"), None), None);
    }
}
