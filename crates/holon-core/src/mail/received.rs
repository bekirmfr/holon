//! Mail received (host-capabilities L4 §5.2.4): a message's bytes, RFC 5322 and MIME, as a flow's
//! `on.mail` handler has it, `m`. A pure function of the bytes and of whose verdict on the sender
//! counts, so the host, `holon verify` and the editor's replay read one message one way.
//!
//! - **Headers** unfolded, RFC 2047's encoded words decoded, addresses read from their lists
//!   (groups, quoted names and comments included).
//! - **Parts** walked down from the top: the first `text/plain` that isn't an attachment is the
//!   text (others like it after it joined on), the first `text/html` the html, and every other
//!   part an attachment, by the name it gives (RFC 2231's too), else `attachment` with its type's
//!   extension. Transfer encodings: 7bit, 8bit, binary, quoted-printable, base64 (read leniently,
//!   as mail readers do). Charsets: UTF-8, US-ASCII, ISO-8859-1, Windows-1252; another as UTF-8.
//! - **The verdict:** only the first `Authentication-Results` header whose `authserv-id` is the
//!   binding's (`--verdict-from`) is read: the mailbox's server writes its own on top as the mail
//!   comes in, so one further down, or under another name, is a sender's and never believed.
//!   `authenticated` is its `dmarc=pass` for the domain of the one address in `From:`.

use std::collections::BTreeMap;

use crate::Value;

/// How deep parts nest before what is further in is left out.
const MAX_DEPTH: usize = 20;

/// The message `raw` as `on.mail` receives it: `from`, `name`, `to`, `cc`, `subject`, `date`
/// (when it is one), `message_id`, `text`, `html` (when there is one), `attachments`,
/// `authenticated` and `verdict`, the last two as the `Authentication-Results` header written by
/// `verdict_from` says (none believed when it is empty).
pub fn received(raw: &[u8], verdict_from: &str) -> Value {
    let (headers, body) = split(raw);
    let field = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    let from = addresses(field("from").unwrap_or(""));
    let (name, sender) = match from.as_slice() {
        [(name, address)] => (name.clone(), Some(address.clone())),
        _ => (String::new(), None),
    };
    let list = |name: &str| Value::List(headers.iter().filter(|(n, _)| n == name).flat_map(|(_, v)| addresses(v)).map(|(_, a)| Value::Text(a)).collect());
    let message_id = field("message-id")
        .and_then(|v| {
            let v = v.trim();
            let id = v.strip_prefix('<').and_then(|r| r.split_once('>')).map_or(v, |(id, _)| id).trim();
            (!id.is_empty()).then(|| id.to_string())
        })
        .unwrap_or_else(|| format!("sha256:{}", crate::crypto::sha256(raw, "hex").expect("hex")));
    let mut found = Found::default();
    walk(&headers, body, 0, &mut found);
    let verdict = verdict(&headers, verdict_from);
    let authenticated = match (&sender, &verdict) {
        (Some(address), Some(v)) => {
            let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
            v.dmarc == "pass" && v.dmarc_from.as_deref().is_none_or(|d| d.eq_ignore_ascii_case(domain))
        }
        _ => false,
    };
    let result = |r: Option<&str>| Value::Text(r.unwrap_or("none").to_string());
    let mut m = BTreeMap::from([
        ("name".to_string(), Value::Text(name)),
        ("to".to_string(), list("to")),
        ("cc".to_string(), list("cc")),
        ("subject".to_string(), Value::Text(words(field("subject").unwrap_or("")).trim().to_string())),
        ("message_id".to_string(), Value::Text(message_id)),
        ("text".to_string(), Value::Text(found.text.unwrap_or_default())),
        ("attachments".to_string(), Value::Record(found.attachments)),
        ("authenticated".to_string(), Value::Boolean(authenticated)),
        (
            "verdict".to_string(),
            Value::Record(BTreeMap::from([
                ("spf".to_string(), result(verdict.as_ref().and_then(|v| v.spf.as_deref()))),
                ("dkim".to_string(), result(verdict.as_ref().and_then(|v| v.dkim.as_deref()))),
                ("dmarc".to_string(), result(verdict.as_ref().map(|v| v.dmarc.as_str()).filter(|d| !d.is_empty()))),
            ])),
        ),
    ]);
    if let Some(s) = sender {
        m.insert("from".into(), Value::Text(s));
    }
    if let Some(d) = field("date").and_then(date) {
        m.insert("date".into(), Value::Number(d as f64));
    }
    if let Some(h) = found.html {
        m.insert("html".into(), Value::Text(h));
    }
    Value::Record(m)
}

/// What the walk down the parts found.
#[derive(Default)]
struct Found {
    text: Option<String>,
    html: Option<String>,
    attachments: BTreeMap<String, Value>,
}

/// An entity's headers (names in lowercase, values unfolded, in order) and its body: the headers
/// end at the first empty line; without one, it is all headers.
fn split(raw: &[u8]) -> (Vec<(String, String)>, &[u8]) {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while at < raw.len() {
        let end = raw[at..].iter().position(|&b| b == b'\n').map_or(raw.len(), |p| at + p + 1);
        let line = &raw[at..end];
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        at = end;
        if content.is_empty() {
            return (headers, &raw[at..]);
        }
        let text = String::from_utf8_lossy(content);
        if content[0] == b' ' || content[0] == b'\t' {
            if let Some((_, v)) = headers.last_mut() {
                v.push_str(&text);
            }
            continue;
        }
        if let Some((name, value)) = text.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim_start().to_string()));
        }
    }
    (headers, &raw[raw.len()..])
}

/// Walk an entity: a multipart's parts, in order; a message's text, html or attachment.
fn walk(headers: &[(String, String)], body: &[u8], depth: usize, found: &mut Found) {
    let field = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    let (kind, params) = parameters(field("content-type").unwrap_or("text/plain"));
    let kind = if kind.contains('/') { kind } else { "text/plain".to_string() };
    if let Some(boundary) = params.get("boundary").filter(|_| kind.starts_with("multipart/") && depth < MAX_DEPTH) {
        for part in parts(body, boundary) {
            let (h, b) = split(part);
            walk(&h, b, depth + 1, found);
        }
        return;
    }
    let (disposition, disposition_params) = parameters(field("content-disposition").unwrap_or(""));
    let named = disposition_params.get("filename").or_else(|| params.get("name")).map(|n| base_name(n)).filter(|n| !n.is_empty());
    let bytes = transfer_decoded(body, field("content-transfer-encoding").unwrap_or(""));
    let charset = params.get("charset").map(|c| c.to_ascii_lowercase());
    let attachment = disposition == "attachment" || named.is_some();
    if !attachment && kind == "text/plain" {
        let text = lines(&decoded(&bytes, charset.as_deref()));
        match &mut found.text {
            Some(t) => *t += &text,
            None => found.text = Some(text),
        }
        return;
    }
    if !attachment && kind == "text/html" && found.html.is_none() {
        found.html = Some(lines(&decoded(&bytes, charset.as_deref())));
        return;
    }
    // A multipart without its boundary, or nested past what is read.
    if !attachment && kind.starts_with("multipart/") {
        return;
    }
    let name = named.unwrap_or_else(|| format!("attachment.{}", extension(&kind)));
    let content = attachment_content(&kind, &name, charset.as_deref(), bytes);
    let mut unique = name.clone();
    let mut n = 2;
    while found.attachments.contains_key(&unique) {
        unique = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
            _ => format!("{name} ({n})"),
        };
        n += 1;
    }
    found.attachments.insert(unique, content);
}

/// A multipart body's parts: what lies between its `--boundary` lines, up to `--boundary--`.
fn parts<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let mut found = Vec::new();
    let mut start: Option<usize> = None;
    let mut at = 0;
    while at < body.len() {
        let end = body[at..].iter().position(|&b| b == b'\n').map_or(body.len(), |p| at + p + 1);
        let line = &body[at..end];
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let trimmed = content.trim_ascii_end();
        if let Some(rest) = trimmed.strip_prefix(delimiter.as_bytes())
            && (rest.is_empty() || rest == b"--")
        {
            if let Some(s) = start {
                // The line break before a delimiter belongs to it.
                let mut part_end = at;
                if body[..part_end].ends_with(b"\n") {
                    part_end -= 1;
                    if body[..part_end].ends_with(b"\r") {
                        part_end -= 1;
                    }
                }
                found.push(&body[s..part_end.max(s)]);
            }
            if rest == b"--" {
                return found;
            }
            start = Some(end);
        }
        at = end;
    }
    // No closing delimiter: the last part runs to the end.
    if let Some(s) = start {
        found.push(&body[s..]);
    }
    found
}

/// A header's value and its parameters, `type/subtype; name="value"`: the value in lowercase, the
/// parameters' names in lowercase, quoted values unquoted, RFC 2231's continued and encoded ones
/// (`name*0*=UTF-8''…`, `name*1=…`) put together and decoded, and RFC 2047's words in a value
/// decoded (as mailers write file names).
fn parameters(header: &str) -> (String, BTreeMap<String, String>) {
    let mut pieces = split_outside_quotes(header, ';').into_iter();
    let value = uncomment(&pieces.next().unwrap_or_default()).trim().to_ascii_lowercase();
    let mut plain: BTreeMap<String, String> = BTreeMap::new();
    // RFC 2231's pieces, by name: (number, whether encoded, value).
    let mut continued: BTreeMap<String, Vec<(u32, bool, String)>> = BTreeMap::new();
    for piece in pieces {
        let Some((name, v)) = piece.split_once('=') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let v = v.trim();
        let v = match v.strip_prefix('"') {
            Some(rest) => unquoted(rest),
            None => uncomment(v).trim().to_string(),
        };
        match name.split_once('*') {
            None => {
                plain.entry(name).or_insert(v);
            }
            Some((base, rest)) => {
                let encoded = rest.ends_with('*') || rest.is_empty();
                let number = rest.trim_end_matches('*').parse::<u32>().unwrap_or(0);
                continued.entry(base.to_string()).or_default().push((number, encoded, v));
            }
        }
    }
    for (name, mut pieces) in continued {
        pieces.sort_by_key(|p| p.0);
        let mut charset = String::new();
        let mut bytes = Vec::new();
        for (i, (_, encoded, v)) in pieces.iter().enumerate() {
            if !*encoded {
                bytes.extend_from_slice(v.as_bytes());
                continue;
            }
            let mut v = v.as_str();
            if i == 0 {
                let mut it = v.splitn(3, '\'');
                if let (Some(c), Some(_language), Some(rest)) = (it.next(), it.next(), it.next()) {
                    charset = c.to_ascii_lowercase();
                    v = rest;
                }
            }
            bytes.extend(percent_decoded(v));
        }
        plain.insert(name, decoded(&bytes, Some(&charset)));
    }
    let plain = plain.into_iter().map(|(k, v)| (k, if v.contains("=?") { words(&v) } else { v })).collect();
    (value, plain)
}

/// `s` cut at each `sep` that isn't inside a quoted string.
fn split_outside_quotes(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let (mut quoted, mut escaped) = (false, false);
    for c in s.chars() {
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if c == sep && !quoted {
            out.push(String::new());
            continue;
        }
        out.last_mut().expect("one").push(c);
    }
    out
}

/// A quoted string's content, after its opening quote, to its closing one: `\x` is `x`.
fn unquoted(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '"' => break,
            c => out.push(c),
        }
    }
    out
}

/// `s` without its comments, `(…)`, nested or not, outside quoted strings.
fn uncomment(s: &str) -> String {
    let mut out = String::new();
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for c in s.chars() {
        if escaped {
            escaped = false;
            if depth == 0 {
                out.push(c);
            }
            continue;
        }
        match c {
            '\\' if quoted || depth > 0 => {
                escaped = true;
                if depth == 0 {
                    out.push(c);
                }
            }
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push(c);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => depth -= 1,
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The addresses an address list holds, each with its display name (`""` when none): groups'
/// members (`Team: a@x, b@y;`), `Name <a@x>`, `a@x (Name)` and a bare `a@x`. What isn't an address
/// (no `@`, a space or a control character in it) is left out.
fn addresses(list: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let list = uncomment(list);
    let mut pieces = Vec::new();
    let (mut piece, mut quoted, mut escaped, mut angled) = (String::new(), false, false, false);
    for c in list.chars() {
        if escaped {
            escaped = false;
            piece.push(c);
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => angled = true,
            '>' if !quoted => angled = false,
            // A group's name ends at its colon; its members at its semicolon.
            ':' if !quoted && !angled => {
                piece.clear();
                continue;
            }
            ',' | ';' if !quoted && !angled => {
                pieces.push(std::mem::take(&mut piece));
                continue;
            }
            _ => {}
        }
        piece.push(c);
    }
    pieces.push(piece);
    for p in pieces {
        let p = p.trim();
        let (name, address) = match p.rfind('<') {
            Some(open) => {
                let address = p[open + 1..].split('>').next().unwrap_or("").trim();
                (words(&unquote_phrase(p[..open].trim())), address)
            }
            None => (String::new(), p),
        };
        // `<@relay:a@x>`, a source route, names the address after its colon.
        let address = address.rsplit(':').next().unwrap_or(address).trim();
        if address.contains('@') && !address.starts_with('@') && !address.ends_with('@') && !address.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | ',')) {
            out.push((name.trim().to_string(), address.to_string()));
        }
    }
    out
}

/// A display name as written, its quotes taken off (`"Doe, Jane"` is `Doe, Jane`).
fn unquote_phrase(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {}
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// A header's text with RFC 2047's encoded words decoded (`=?UTF-8?B?…?=`, `=?ISO-8859-1?Q?…?=`),
/// the space between two of them dropped.
fn words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    let mut after_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        let Some((word, after)) = encoded_word(tail) else {
            out += before;
            out += "=?";
            rest = &tail[2..];
            after_word = false;
            continue;
        };
        if !(after_word && before.chars().all(char::is_whitespace)) {
            out += before;
        }
        out += &word;
        rest = after;
        after_word = true;
    }
    out + rest
}

/// The encoded word `s` starts with, decoded, and what follows it.
fn encoded_word(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix("=?")?;
    let (charset, body) = body.split_once('?')?;
    let (encoding, body) = body.split_once('?')?;
    let end = body.find("?=")?;
    let (text, after) = (&body[..end], &body[end + 2..]);
    if text.contains(char::is_whitespace) || charset.contains(char::is_whitespace) {
        return None;
    }
    // RFC 2231's language, `UTF-8*en`, says nothing about the bytes.
    let charset = charset.split('*').next().unwrap_or(charset).to_ascii_lowercase();
    let bytes = match encoding {
        "B" | "b" => base64_lenient(text.as_bytes()),
        "Q" | "q" => {
            let mut out = Vec::new();
            let b = text.as_bytes();
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'_' => out.push(b' '),
                    b'=' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                        out.push(hex(b[i + 1])? << 4 | hex(b[i + 2])?);
                        i += 2;
                    }
                    c => out.push(c),
                }
                i += 1;
            }
            out
        }
        _ => return None,
    };
    Some((decoded(&bytes, Some(&charset)), after))
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// `%XX` decoded, as RFC 2231 writes a parameter's bytes.
fn percent_decoded(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// A body's bytes, its transfer encoding undone: base64 and quoted-printable read as mail readers
/// do (what doesn't belong skipped, a lone `=` kept); anything else as it is.
fn transfer_decoded(body: &[u8], encoding: &str) -> Vec<u8> {
    match uncomment(encoding).trim().to_ascii_lowercase().as_str() {
        "base64" => base64_lenient(body),
        "quoted-printable" => {
            let mut out = Vec::with_capacity(body.len());
            let mut i = 0;
            while i < body.len() {
                if body[i] != b'=' {
                    out.push(body[i]);
                    i += 1;
                    continue;
                }
                // A soft line break: `=`, spaces it may have picked up, then the line's end.
                let mut j = i + 1;
                while j < body.len() && (body[j] == b' ' || body[j] == b'\t') {
                    j += 1;
                }
                if body.get(j) == Some(&b'\n') {
                    i = j + 1;
                } else if body.get(j) == Some(&b'\r') && body.get(j + 1) == Some(&b'\n') {
                    i = j + 2;
                } else if let (Some(h), Some(l)) = (body.get(i + 1).copied().and_then(hex), body.get(i + 2).copied().and_then(hex)) {
                    out.push(h << 4 | l);
                    i += 3;
                } else {
                    out.push(b'=');
                    i += 1;
                }
            }
            out
        }
        _ => body.to_vec(),
    }
}

/// Base64 read leniently: every character outside its alphabet skipped, decoding stopped at `=`,
/// leftover bits dropped.
fn base64_lenient(s: &[u8]) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s {
        if c == b'=' {
            break;
        }
        let Some(v) = value(c) else { continue };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Bytes in `charset` as a text: UTF-8 and US-ASCII (what isn't UTF-8 as U+FFFD), ISO-8859-1,
/// Windows-1252; any other as UTF-8. A byte order mark at the start is dropped.
fn decoded(bytes: &[u8], charset: Option<&str>) -> String {
    let text = match charset.unwrap_or("utf-8") {
        "iso-8859-1" | "iso8859-1" | "latin1" | "latin-1" | "l1" => bytes.iter().map(|&b| b as char).collect(),
        "windows-1252" | "cp1252" | "x-cp1252" => bytes.iter().map(|&b| windows_1252(b)).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text)
}

/// A Windows-1252 byte's character: ISO-8859-1's, but for 0x80 to 0x9F.
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{9d}', 'ž', 'Ÿ',
    ];
    if (0x80..0xa0).contains(&b) { HIGH[(b - 0x80) as usize] } else { b as char }
}

/// A body's text with its lines ending as a flow's texts do: `\n`.
fn lines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// An attachment's content: a text when its type is one (`text/*`, JSON, XML, CSV) in a charset
/// given or in UTF-8, or when its name says CSV, text, JSON or XML and it is UTF-8; else its bytes.
fn attachment_content(kind: &str, name: &str, charset: Option<&str>, bytes: Vec<u8>) -> Value {
    let typed = kind.starts_with("text/") || matches!(kind, "application/json" | "application/xml" | "application/csv");
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let by_name = matches!(ext.as_str(), "csv" | "tsv" | "txt" | "json" | "xml");
    match charset {
        Some(c) if typed => Value::Text(decoded(&bytes, Some(c))),
        _ if typed || by_name => match String::from_utf8(bytes) {
            Ok(t) => Value::Text(t.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(t)),
            Err(e) if typed => Value::Text(decoded(e.as_bytes(), Some("windows-1252"))),
            Err(e) => Value::Bytes(e.into_bytes()),
        },
        _ => Value::Bytes(bytes),
    }
}

/// A file name without the folders a sender may have put before it.
fn base_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.chars().filter(|c| !c.is_control() && *c != '"').collect::<String>().trim().to_string()
}

/// A type's usual extension, for an attachment that gives no name.
fn extension(kind: &str) -> &'static str {
    match kind {
        "text/csv" | "application/csv" => "csv",
        "text/plain" => "txt",
        "text/html" => "html",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "message/rfc822" => "eml",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => "bin",
    }
}

/// RFC 5322's `Date:` in seconds since the epoch: `[Sun, ]8 Nov 2026 21:44[:00] +0300`, a two- or
/// three-digit year as RFC 5322's obsolete syntax reads it, the obsolete zones (`GMT`, `EST`, …;
/// another letter or name as `+0000`), comments left out.
fn date(s: &str) -> Option<i64> {
    let s = uncomment(s);
    let s = s.split_once(',').map_or(s.as_str(), |(_, rest)| rest);
    let mut tokens = s.split_whitespace();
    let day: i64 = tokens.next()?.parse().ok()?;
    let month = tokens.next()?.to_ascii_lowercase();
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| month.starts_with(m))? as i64 + 1;
    let year_text = tokens.next()?;
    let year: i64 = year_text.parse().ok()?;
    let year = match year_text.len() {
        2 if year < 50 => 2000 + year,
        2 | 3 => 1900 + year,
        _ => year,
    };
    let mut time = tokens.next()?.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next().map_or(Some(0), |s| s.parse().ok())?;
    let zone = tokens.next().unwrap_or("+0000");
    let offset = match zone.as_bytes().first() {
        Some(b'+' | b'-') if zone.len() == 5 && zone[1..].bytes().all(|b| b.is_ascii_digit()) => {
            let n: i64 = zone[1..].parse().ok()?;
            let minutes = n / 100 * 60 + n % 100;
            if zone.starts_with('-') { -minutes } else { minutes }
        }
        _ => match zone.to_ascii_uppercase().as_str() {
            "EDT" => -4 * 60,
            "EST" | "CDT" => -5 * 60,
            "CST" | "MDT" => -6 * 60,
            "MST" | "PDT" => -7 * 60,
            "PST" => -8 * 60,
            _ => 0,
        },
    };
    if !(1..=31).contains(&day) || !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }
    let days = crate::dates::days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset * 60)
}

/// What a mailbox's server says of a message's sender, as its `Authentication-Results` writes it.
struct Verdict {
    spf: Option<String>,
    dkim: Option<String>,
    /// `""` when it gives no DMARC result.
    dmarc: String,
    /// The domain its DMARC result is for (`header.from`), when it says.
    dmarc_from: Option<String>,
}

/// The verdict of the first `Authentication-Results` header whose `authserv-id` is `by`
/// (letters compared without case): none for an empty `by`, or when there is no such header.
/// Of several DKIM results, a `pass` is the one taken.
fn verdict(headers: &[(String, String)], by: &str) -> Option<Verdict> {
    let by = by.trim();
    if by.is_empty() {
        return None;
    }
    let value = headers.iter().filter(|(n, _)| n == "authentication-results").map(|(_, v)| uncomment(v)).find(|v| {
        let id = v.split(';').next().unwrap_or("").split_whitespace().next().unwrap_or("");
        id.eq_ignore_ascii_case(by)
    })?;
    let mut verdict = Verdict { spf: None, dkim: None, dmarc: String::new(), dmarc_from: None };
    for resinfo in value.split(';').skip(1) {
        let mut tokens = resinfo.split_whitespace();
        let Some((method, result)) = tokens.next().and_then(|t| t.split_once('=')) else { continue };
        let (method, result) = (method.to_ascii_lowercase(), result.trim_matches('"').to_ascii_lowercase());
        match method.as_str() {
            "spf" if verdict.spf.is_none() => verdict.spf = Some(result),
            "dkim" if verdict.dkim.as_deref() != Some("pass") => verdict.dkim = Some(result),
            "dmarc" if verdict.dmarc.is_empty() => {
                verdict.dmarc = result;
                verdict.dmarc_from = tokens.filter_map(|t| t.split_once('=')).find(|(k, _)| k.eq_ignore_ascii_case("header.from")).map(|(_, v)| v.trim_matches('"').to_string());
            }
            _ => {}
        }
    }
    Some(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Value, k: &str) -> String {
        match v {
            Value::Record(r) => match r.get(k) {
                Some(Value::Text(t)) => t.clone(),
                other => panic!("{k}: {other:?}"),
            },
            _ => panic!("not a record"),
        }
    }

    fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
        match v {
            Value::Record(r) => r.get(k),
            _ => None,
        }
    }

    fn texts(v: &Value, k: &str) -> Vec<String> {
        match field(v, k) {
            Some(Value::List(xs)) => xs.iter().map(|x| if let Value::Text(t) = x { t.clone() } else { panic!() }).collect(),
            other => panic!("{k}: {other:?}"),
        }
    }

    const REMITTANCE: &str = "Return-Path: <billing@customer.example>\r\n\
Authentication-Results: mx.acme.com;\r\n\
\tspf=pass (sender IP is 192.0.2.1) smtp.mailfrom=customer.example;\r\n\
\tdkim=pass header.d=customer.example;\r\n\
\tdmarc=pass (p=reject) header.from=customer.example\r\n\
Authentication-Results: mx.acme.com; dmarc=fail header.from=customer.example\r\n\
From: \"Billing, Customer\" <billing@customer.example>\r\n\
To: Remittances <remittances@acme.com>, ops@acme.com\r\n\
Cc: undisclosed-recipients:;\r\n\
Subject: =?UTF-8?B?UmVtaXR0YW5jZSDigJQ=?= =?UTF-8?Q?_November?=\r\n\
Date: Sun, 8 Nov 2026 21:44:00 +0300 (EAT)\r\n\
Message-ID: <remit-42@customer.example>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
preamble, never read\r\n\
--outer\r\n\
Content-Type: multipart/alternative; boundary=inner\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain; charset=iso-8859-1\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Pay=E9 two invoices, see the file attached. A line long enough to be bro=\r\n\
ken.\r\n\
--inner\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Payé</p>\r\n\
--inner--\r\n\
--outer\r\n\
Content-Type: text/csv; name=\"remit.csv\"\r\n\
Content-Disposition: attachment; filename=\"remit.csv\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
aW52b2ljZSxhbW91bnQK\r\n\
SU5WLTEsMTAwLjAwCklOVi0yLDI1MC41MAo=\r\n\
--outer\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename*=UTF-8''re%C3%A7u.pdf\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0=\r\n\
--outer\r\n\
Content-Type: text/csv; name=\"remit.csv\"\r\n\
\r\n\
second\r\n\
--outer--\r\n\
epilogue, never read\r\n";

    /// A remittance as a customer's mailer writes it: every field read, the parts found, the
    /// attachments by name, the verdict the mailbox's server wrote first believed.
    #[test]
    fn a_message_is_read_as_on_mail_receives_it() {
        let m = received(REMITTANCE.as_bytes(), "mx.acme.com");
        assert_eq!(text(&m, "from"), "billing@customer.example");
        assert_eq!(text(&m, "name"), "Billing, Customer");
        assert_eq!(texts(&m, "to"), ["remittances@acme.com", "ops@acme.com"]);
        assert_eq!(texts(&m, "cc"), Vec::<String>::new());
        assert_eq!(text(&m, "subject"), "Remittance — November");
        assert_eq!(field(&m, "date"), Some(&Value::Number(1_794_163_440.0)));
        assert_eq!(text(&m, "message_id"), "remit-42@customer.example");
        assert_eq!(text(&m, "text"), "Payé two invoices, see the file attached. A line long enough to be broken.");
        assert_eq!(text(&m, "html"), "<p>Payé</p>");
        let Some(Value::Record(files)) = field(&m, "attachments") else { panic!() };
        assert_eq!(files.keys().collect::<Vec<_>>(), ["remit (2).csv", "remit.csv", "reçu.pdf"]);
        assert_eq!(files["remit.csv"], Value::Text("invoice,amount\nINV-1,100.00\nINV-2,250.50\n".into()));
        assert_eq!(files["remit (2).csv"], Value::Text("second".into()));
        assert_eq!(files["reçu.pdf"], Value::Bytes(b"%PDF-".to_vec()));
        assert_eq!(field(&m, "authenticated"), Some(&Value::Boolean(true)));
        let Some(Value::Record(v)) = field(&m, "verdict") else { panic!() };
        assert_eq!((&v["spf"], &v["dkim"], &v["dmarc"]), (&Value::Text("pass".into()), &Value::Text("pass".into()), &Value::Text("pass".into())));
    }

    /// Only the mailbox's own server is believed: with no `--verdict-from`, under another name,
    /// with DMARC failing or for another domain, or with no single sender, nothing is
    /// authenticated.
    #[test]
    fn a_verdict_is_believed_only_from_the_mailboxs_server() {
        let not = |raw: &str, by: &str| field(&received(raw.as_bytes(), by), "authenticated") == Some(&Value::Boolean(false));
        assert!(not(REMITTANCE, ""));
        assert!(not(REMITTANCE, "mx.evil.example"));
        let forged = "Authentication-Results: mx.evil.example; dmarc=pass header.from=customer.example\r\nAuthentication-Results: mx.acme.com; dmarc=fail\r\nFrom: billing@customer.example\r\n\r\nhi";
        assert!(!not(forged, "mx.evil.example"), "the header the named server wrote is the one read");
        assert!(not(forged, "mx.acme.com"));
        let other_domain = "Authentication-Results: mx.acme.com; dmarc=pass header.from=evil.example\r\nFrom: billing@customer.example\r\n\r\nhi";
        assert!(not(other_domain, "mx.acme.com"));
        let two_senders = "Authentication-Results: mx.acme.com; dmarc=pass\r\nFrom: a@customer.example, b@customer.example\r\n\r\nhi";
        assert!(not(two_senders, "mx.acme.com"));
        let m = received(two_senders.as_bytes(), "mx.acme.com");
        assert_eq!(field(&m, "from"), None, "no single sender: no from");
        let none = received(b"From: a@b.example\r\n\r\nhi", "mx.acme.com");
        let Some(Value::Record(v)) = field(&none, "verdict") else { panic!() };
        assert_eq!(v["dmarc"], Value::Text("none".into()));
    }

    /// What a message lacks reads as nothing, and bytes that aren't a message still read as one:
    /// no `Message-ID` is its hash, no text is `""`, a date that isn't one is left out, and
    /// whatever bytes come, reading them never fails.
    #[test]
    fn whatever_bytes_come_they_read_as_a_message() {
        let m = received(b"Subject: hi\nDate: someday\n\nplain text, LF lines\nsecond\n", "");
        assert_eq!(text(&m, "text"), "plain text, LF lines\nsecond\n");
        assert_eq!(field(&m, "date"), None);
        assert!(text(&m, "message_id").starts_with("sha256:"));
        assert_eq!(field(&m, "from"), None);
        let empty = received(b"", "");
        assert_eq!(text(&empty, "text"), "");
        let mut seed = 0x1234_5678u64;
        let table = b"=?:;<>\"()\r\n -boundary\tAbc@.";
        for len in 0..300 {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    table[(seed >> 33) as usize % table.len()]
                })
                .collect();
            let _ = received(&bytes, "x");
        }
        let nested = format!("Content-Type: multipart/mixed; boundary=b\r\n\r\n{}", "--b\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n".repeat(40));
        let _ = received(nested.as_bytes(), "");
    }

    /// Charsets and dates as mailers write them.
    #[test]
    fn charsets_and_dates_read_as_written() {
        assert_eq!(words("=?windows-1252?Q?=80_5?= and =?ISO-8859-1?B?6Q==?="), "€ 5 and é");
        assert_eq!(words("=?utf-8?q?a?=  =?utf-8?q?b?= c"), "ab c");
        assert_eq!(words("=?bogus"), "=?bogus");
        assert_eq!(date("8 Nov 2026 18:44 GMT"), Some(1_794_163_440));
        assert_eq!(date("Sun, 08 Nov 26 13:44:00 EST"), Some(1_794_163_440));
        assert_eq!(date("Sun, 32 Nov 2026 13:44:00 +0000"), None);
        let (kind, params) = parameters("Text/Plain; CHARSET=\"utf-8\"; name*0*=UTF-8''r%C3%A9; name*1=\"mit.csv\" (a comment)");
        assert_eq!((kind.as_str(), params["charset"].as_str(), params["name"].as_str()), ("text/plain", "utf-8", "rémit.csv"));
        assert_eq!(addresses("Jane <jane@x.example>, \"Doe, J\" <j@x.example>, k@x.example (Kay), Team: a@x.example, b@x.example;, nobody"), [
            ("Jane".to_string(), "jane@x.example".to_string()),
            ("Doe, J".into(), "j@x.example".into()),
            ("".into(), "k@x.example".into()),
            ("".into(), "a@x.example".into()),
            ("".into(), "b@x.example".into()),
        ]);
    }
}
