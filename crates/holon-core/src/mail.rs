//! Mail (host-capabilities L4 §5.2.2): what `mail_send` may send, as pure checks the lowering,
//! core and the host share.
//!
//! A flow sends through a mail resource an admin binds (`import ops from "mail:ops"`,
//! `ops.send({ to, subject, text })`). Its recipients are part of the grant: the leaf's `to`
//! parameter is the ones the text names, each written in quotes, or `*` when one is computed, and
//! the capability names each, `MailSend("ops/approvals@acme.com")` or `MailSend("ops/*")`. The
//! effect a run performs carries `MailSend("ops")` alone, its recipients in its payload, which the
//! host holds to the grant and to the binding's `--to` as each mail goes ([`within`]).

use crate::{Capability, Param, Value};

/// The most recipients one mail has, `to` and `cc` together.
pub const MAX_RECIPIENTS: usize = 50;
/// The most one mail's subject, texts and attachments come to, before the host encodes it.
pub const MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;
/// The most attachments one mail has.
pub const MAX_ATTACHMENTS: usize = 100;
/// The most mails one run sends.
pub const MAX_MAILS_PER_RUN: usize = 100;

/// The fields a message has: `to` (required), `cc`, `subject` (required), `text` (required),
/// `html`, `attachments`.
pub const FIELDS: &[&str] = &["to", "cc", "subject", "text", "html", "attachments"];

/// An address a mail may go to: `local@domain`, ASCII, as SMTP's `RCPT TO` takes it without
/// quoting. The local part holds no `/` (the capability's separator), space, comma or angle
/// bracket; the domain is DNS labels.
pub fn is_address(s: &str) -> bool {
    let Some((local, domain)) = s.rsplit_once('@') else { return false };
    s.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && local.bytes().all(|b| b.is_ascii_graphic() && !b"<>()[]\\,;:\"@/".contains(&b))
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && is_domain(domain)
}

fn is_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// A pattern of a binding's `--to`: an address, `*@domain` (any address there), or `*`.
pub fn is_pattern(p: &str) -> bool {
    p == "*" || p.strip_prefix("*@").is_some_and(is_domain) || is_address(p)
}

/// Whether `address` is one `pattern` allows, letters compared without case.
pub fn within(address: &str, pattern: &str) -> bool {
    match pattern.strip_prefix("*@") {
        _ if pattern == "*" => true,
        Some(domain) => address.rsplit_once('@').is_some_and(|(_, d)| d.eq_ignore_ascii_case(domain)),
        None => address.eq_ignore_ascii_case(pattern),
    }
}

/// The recipients a `mail_send`'s `to` parameter names: `*`, or addresses separated by commas,
/// one at least and [`MAX_RECIPIENTS`] at most, each once.
pub fn recipients(to: &str) -> Option<Vec<String>> {
    if to == "*" {
        return Some(vec!["*".into()]);
    }
    let mut out: Vec<String> = Vec::new();
    for a in to.split(',').map(str::trim) {
        if !is_address(a) {
            return None;
        }
        if !out.iter().any(|o| o.eq_ignore_ascii_case(a)) {
            out.push(a.to_string());
        }
    }
    (!out.is_empty() && out.len() <= MAX_RECIPIENTS).then_some(out)
}

/// A `mail_send`'s capabilities: one for each recipient its `to` names, under the mail's name
/// (`MailSend("ops/approvals@acme.com")`, `MailSend("ops/*")`). Without `to`, the effect a run
/// performs, `MailSend("ops")`, its recipients its payload's.
pub(crate) fn capabilities(params: &[Param]) -> Option<Vec<Capability>> {
    let mail = crate::kind::ident(params, "mail")?;
    let Some(to) = params.iter().find(|p| p.name() == "to") else {
        return Some(vec![Capability::MailSend(mail)]);
    };
    Some(recipients(to.value()?)?.into_iter().map(|r| Capability::MailSend(format!("{mail}/{r}"))).collect())
}

/// The message a `mail_send` sends, made regular: `to` and `cc` lists of addresses (`cc` only when
/// there is one), `subject` and `text`, and `html` and `attachments` when given. Nothing for a
/// message that isn't one: a field it doesn't know, no recipient or past [`MAX_RECIPIENTS`], an
/// address that isn't one or that `to` (the recipients the text names) doesn't name, a subject
/// with a line break in it, an attachment that isn't a text or bytes or whose name isn't a file's,
/// or past [`MAX_MESSAGE_BYTES`].
pub(crate) fn message(message: &Value, to: Option<&str>) -> Option<Value> {
    let Value::Record(m) = message else { return None };
    if !m.keys().all(|k| FIELDS.contains(&k.as_str())) {
        return None;
    }
    let addresses = |k: &str| -> Option<Vec<String>> {
        match m.get(k) {
            None => Some(Vec::new()),
            Some(Value::Text(t)) => Some(vec![t.trim().to_string()]),
            Some(Value::List(xs)) => xs.iter().map(|x| if let Value::Text(t) = x { Some(t.trim().to_string()) } else { None }).collect(),
            _ => None,
        }
    };
    let (sent_to, cc) = (addresses("to")?, addresses("cc")?);
    let all: Vec<&String> = sent_to.iter().chain(&cc).collect();
    if sent_to.is_empty() || all.len() > MAX_RECIPIENTS || !all.iter().all(|a| is_address(a)) {
        return None;
    }
    if let Some(named) = to.filter(|t| *t != "*") {
        let named = recipients(named)?;
        if !all.iter().all(|a| named.iter().any(|n| n.eq_ignore_ascii_case(a))) {
            return None;
        }
    }
    let text = |k: &str| match m.get(k) {
        Some(Value::Text(t)) => Some(t.clone()),
        _ => None,
    };
    let (subject, body) = (text("subject")?, text("text")?);
    if subject.contains(['\r', '\n']) {
        return None;
    }
    let html = match m.get("html") {
        None => None,
        Some(_) => Some(text("html")?),
    };
    let mut size = subject.len() + body.len() + html.as_ref().map_or(0, String::len);
    let attachments = match m.get("attachments") {
        None => None,
        Some(Value::Record(files)) => {
            if files.len() > MAX_ATTACHMENTS {
                return None;
            }
            for (name, content) in files {
                size += match content {
                    Value::Text(t) => t.len(),
                    Value::Bytes(b) => b.len(),
                    _ => return None,
                };
                if !is_file_name(name) {
                    return None;
                }
            }
            Some(Value::Record(files.clone()))
        }
        Some(_) => return None,
    };
    if size > MAX_MESSAGE_BYTES {
        return None;
    }
    let list = |xs: Vec<String>| Value::List(xs.into_iter().map(Value::Text).collect());
    let mut out = std::collections::BTreeMap::from([("to".to_string(), list(sent_to)), ("subject".to_string(), Value::Text(subject)), ("text".to_string(), Value::Text(body))]);
    if !cc.is_empty() {
        out.insert("cc".into(), list(cc));
    }
    if let Some(h) = html {
        out.insert("html".into(), Value::Text(h));
    }
    if let Some(a) = attachments {
        out.insert("attachments".into(), a);
    }
    Some(Value::Record(out))
}

/// A binding's sender, `Treasury <treasury@acme.com>` or `treasury@acme.com`: its name, if it has
/// one, and its address. The name holds no quote, angle bracket or line break.
pub fn sender(s: &str) -> Option<(Option<String>, String)> {
    let s = s.trim();
    let (name, address) = match s.strip_suffix('>').and_then(|r| r.rsplit_once('<')) {
        Some((name, address)) => (Some(name.trim().to_string()).filter(|n| !n.is_empty()), address.trim()),
        None => (None, s),
    };
    if name.as_ref().is_some_and(|n| n.chars().any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '\\'))) || !is_address(address) {
        return None;
    }
    Some((name, address.to_string()))
}

/// The message `mail_send` made regular ([`message`]) as it is sent: RFC 5322's headers and MIME's
/// parts, from `from` (the binding's sender, [`sender`]), dated `date` (seconds since the epoch, the
/// run's clock), known by `id` (its `Message-ID`, without the brackets). Lines end CR LF and none
/// is longer than 78 characters: the texts quoted-printable, the attachments base64, a header that
/// isn't ASCII in RFC 2047's words. No `Bcc`, no `Reply-To`. None for what isn't such a message.
pub fn mime(message: &Value, from: &str, date: i64, id: &str) -> Option<Vec<u8>> {
    let Value::Record(m) = message else { return None };
    let (name, address) = sender(from)?;
    let list = |k: &str| -> Option<String> {
        match m.get(k) {
            Some(Value::List(xs)) => xs.iter().map(|x| if let Value::Text(t) = x { is_address(t).then(|| t.clone()) } else { None }).collect::<Option<Vec<_>>>().map(|v| v.join(", ")),
            _ => None,
        }
    };
    let text = |k: &str| if let Some(Value::Text(t)) = m.get(k) { Some(t.as_str()) } else { None };
    let mut out = String::new();
    out += &match name {
        Some(n) => header("From", &format!("{} <{address}>", phrase(&n))),
        None => format!("From: {address}\r\n"),
    };
    out += &header("To", &list("to")?);
    if m.contains_key("cc") {
        out += &header("Cc", &list("cc")?);
    }
    out += &header("Subject", &encoded(text("subject")?));
    out += &format!("Date: {}\r\n", rfc5322_date(date));
    out += &format!("Message-ID: <{id}>\r\n");
    out += "MIME-Version: 1.0\r\n";
    let plain = format!("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}", quoted_printable(text("text")?));
    let body = match m.get("html") {
        None => plain,
        Some(_) => {
            let html = format!("Content-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}", quoted_printable(text("html")?));
            multipart("alternative", "=_holon_alternative", &[plain, html])
        }
    };
    let body = match m.get("attachments") {
        Some(Value::Record(files)) if !files.is_empty() => {
            let mut parts = vec![body];
            for (name, content) in files {
                let (bytes, kind) = match content {
                    Value::Text(t) => (t.as_bytes(), format!("{}; charset=utf-8", content_type(name))),
                    Value::Bytes(b) => (b.as_slice(), content_type(name).to_string()),
                    _ => return None,
                };
                let named = file_name_parameter(name);
                let mut part = format!("Content-Type: {kind}; name{named}\r\nContent-Disposition: attachment; filename{named}\r\nContent-Transfer-Encoding: base64\r\n\r\n");
                let encoded = crate::crypto::base64_encode(bytes, crate::crypto::alphabet("standard")?, true);
                for line in encoded.as_bytes().chunks(76) {
                    part += std::str::from_utf8(line).ok()?;
                    part += "\r\n";
                }
                parts.push(part);
            }
            multipart("mixed", "=_holon_mixed", &parts)
        }
        _ => body,
    };
    out += &body;
    Some(out.into_bytes())
}

/// A header line, folded where it would pass 78 characters (at a space, before an encoded word
/// or an address).
fn header(name: &str, value: &str) -> String {
    let mut out = format!("{name}:");
    let mut line = out.len();
    for (i, word) in value.split(' ').enumerate() {
        if i > 0 && line + 1 + word.len() > 78 {
            out += "\r\n";
            line = 0;
        }
        out.push(' ');
        out += word;
        line += 1 + word.len();
    }
    out + "\r\n"
}

/// A text as a header's words: as it is when it is printable ASCII, else in RFC 2047's encoded
/// words, base64 of whole characters, each word short enough for a folded line.
fn encoded(text: &str) -> String {
    if text.bytes().all(|b| (0x20..0x7f).contains(&b)) && !text.contains("=?") && text.split(' ').all(|w| w.len() <= 70) {
        return text.to_string();
    }
    let alphabet = crate::crypto::alphabet("standard").expect("standard");
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if chunk.len() + c.len_utf8() > 42 {
            words.push(format!("=?UTF-8?B?{}?=", crate::crypto::base64_encode(chunk.as_bytes(), alphabet, true)));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() || words.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", crate::crypto::base64_encode(chunk.as_bytes(), alphabet, true)));
    }
    words.join(" ")
}

/// A sender's name as a header writes it: quoted when it holds a character an atom can't, in
/// encoded words when it isn't ASCII.
fn phrase(name: &str) -> String {
    if !name.is_ascii() {
        return encoded(name);
    }
    if name.chars().all(|c| c.is_ascii_alphanumeric() || " !#$%&'*+-/=?^_`{|}~".contains(c)) { name.to_string() } else { format!("\"{name}\"") }
}

/// A file name as a MIME parameter, after `name` or `filename`: `="a.csv"`, or RFC 2231's
/// `*=UTF-8''…` for one that isn't ASCII.
fn file_name_parameter(name: &str) -> String {
    if name.is_ascii() {
        return format!("=\"{name}\"");
    }
    let mut out = "*=UTF-8''".to_string();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

/// Parts joined under `boundary`. A boundary with `=_` in it can't be in a quoted-printable or
/// base64 part, which are all the parts there are.
fn multipart(kind: &str, boundary: &str, parts: &[String]) -> String {
    let mut out = format!("Content-Type: multipart/{kind}; boundary=\"{boundary}\"\r\n\r\n");
    for p in parts {
        out += &format!("--{boundary}\r\n{p}");
        if !p.ends_with("\r\n") {
            out += "\r\n";
        }
    }
    out + &format!("--{boundary}--\r\n")
}

/// A text as quoted-printable (RFC 2045): its lines, however they ended, each ending CR LF; `=`,
/// what isn't printable ASCII, and a space or tab ending a line written `=XX`; a line longer than
/// 76 characters broken with a soft `=`.
fn quoted_printable(text: &str) -> String {
    let mut out = String::new();
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for line in text.split('\n') {
        let mut width = 0;
        let bytes = line.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            let last = i + 1 == bytes.len();
            let piece = match b {
                b'=' => format!("={b:02X}"),
                b' ' | b'\t' if last => format!("={b:02X}"),
                b' ' | b'\t' | 33..=126 => (b as char).to_string(),
                _ => format!("={b:02X}"),
            };
            if width + piece.len() > if last { 76 } else { 75 } {
                out += "=\r\n";
                width = 0;
            }
            width += piece.len();
            out += &piece;
        }
        out += "\r\n";
    }
    out
}

/// An attachment's type, by its name's extension.
fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "csv" => "text/csv",
        "txt" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "asc" | "pgp" | "gpg" => "application/pgp-encrypted",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

/// `date` (seconds since the epoch) as RFC 5322 writes it, in UTC: `Sun, 08 Nov 2026 09:30:00 +0000`.
fn rfc5322_date(date: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let [y, m, d, wd, h, min, s, _] = crate::dates::parts(date * 1000);
    format!("{}, {d:02} {} {y} {h:02}:{min:02}:{s:02} +0000", DAYS[wd as usize], MONTHS[m as usize])
}

/// An attachment's name: a file's, without a folder, a control character or a quote.
fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name != "." && name != ".." && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | '"'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn record(fields: &[(&str, Value)]) -> Value {
        Value::Record(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    #[test]
    fn an_address_is_one_smtp_takes_unquoted() {
        for good in ["approvals@acme.com", "a.b+tag@mail.bank.example", "x@localhost", "o'brien@acme.co.uk"] {
            assert!(is_address(good), "{good}");
        }
        for bad in ["", "acme.com", "@acme.com", "a@", "a b@acme.com", "a,b@acme.com", "<a@acme.com>", "a/b@acme.com", ".a@acme.com", "a..b@acme.com", "a@-acme.com", "a@acme..com", "ä@acme.com", "a@acme.com\r\nRCPT"] {
            assert!(!is_address(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_pattern_allows_its_address_its_domain_or_anyone() {
        assert!(within("Approvals@ACME.com", "approvals@acme.com"));
        assert!(within("treasury@acme.com", "*@ACME.COM"));
        assert!(!within("treasury@acme.com.evil.example", "*@acme.com"));
        assert!(!within("acme.com@evil.example", "*@acme.com"));
        assert!(within("anyone@anywhere.example", "*"));
        assert!(is_pattern("*") && is_pattern("*@acme.com") && is_pattern("a@acme.com"));
        assert!(!is_pattern("*@") && !is_pattern("*acme.com") && !is_pattern("a@*.com"));
    }

    #[test]
    fn the_grant_names_each_recipient_the_text_names_or_star() {
        let p = |to: &str| vec![Param::frozen("mail", "ops"), Param::frozen("to", to)];
        assert_eq!(capabilities(&p("a@acme.com, b@acme.com,A@acme.com")), Some(vec![Capability::MailSend("ops/a@acme.com".into()), Capability::MailSend("ops/b@acme.com".into())]));
        assert_eq!(capabilities(&p("*")), Some(vec![Capability::MailSend("ops/*".into())]));
        assert_eq!(capabilities(&[Param::frozen("mail", "ops")]), Some(vec![Capability::MailSend("ops".into())]), "the effect a run performs");
        assert_eq!(capabilities(&p("not an address")), None);
        assert_eq!(capabilities(&p("")), None);
        assert_eq!(capabilities(&[Param::frozen("mail", "o/ps"), Param::frozen("to", "*")]), None, "a mail's name is a name");
    }

    #[test]
    fn a_message_is_made_regular() {
        let m = record(&[("to", text(" a@acme.com ")), ("cc", Value::List(vec![text("b@acme.com")])), ("subject", text("Payout 7")), ("text", text("Approve it"))]);
        let made = message(&m, Some("a@acme.com,b@acme.com")).unwrap();
        assert_eq!(made, record(&[("to", Value::List(vec![text("a@acme.com")])), ("cc", Value::List(vec![text("b@acme.com")])), ("subject", text("Payout 7")), ("text", text("Approve it"))]));
        let files = Value::Record(BTreeMap::from([("payout.csv".into(), text("id,amount\n7,1500\n")), ("scan.pdf".into(), Value::Bytes(vec![0x25, 0x50]))]));
        let with = record(&[("to", text("a@acme.com")), ("subject", text("s")), ("text", text("t")), ("html", text("<p>t</p>")), ("attachments", files.clone())]);
        let made = message(&with, Some("*")).unwrap();
        let Value::Record(made) = made else { panic!() };
        assert_eq!((made.get("html"), made.get("attachments"), made.get("cc")), (Some(&text("<p>t</p>")), Some(&files), None));
    }

    #[test]
    fn a_sender_has_an_address_and_maybe_a_name() {
        assert_eq!(sender("Treasury <treasury@acme.com>"), Some((Some("Treasury".into()), "treasury@acme.com".into())));
        assert_eq!(sender(" treasury@acme.com "), Some((None, "treasury@acme.com".into())));
        assert_eq!(sender("<treasury@acme.com>"), Some((None, "treasury@acme.com".into())));
        for bad in ["Treasury", "Tre\"asury <t@acme.com>", "T\r\nBcc: x <t@acme.com>", "Treasury <not one>"] {
            assert_eq!(sender(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_message_is_written_as_mime_its_lines_short_and_ascii() {
        let m = message(&record(&[("to", text("a@acme.com")), ("cc", Value::List(vec![text("b@acme.com")])), ("subject", text("Payout 7")), ("text", text("Approve it: https://x.example/approve/abc\nThanks =)\n"))]), Some("*")).unwrap();
        let sent = String::from_utf8(mime(&m, "Treasury <treasury@acme.com>", 1_794_130_200, "run-1a2b.0@acme.com").unwrap()).unwrap();
        assert_eq!(
            sent,
            "From: Treasury <treasury@acme.com>\r\nTo: a@acme.com\r\nCc: b@acme.com\r\nSubject: Payout 7\r\nDate: Sun, 08 Nov 2026 09:30:00 +0000\r\nMessage-ID: <run-1a2b.0@acme.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nApprove it: https://x.example/approve/abc\r\nThanks =3D)\r\n\r\n"
        );
        // Not ASCII, long, with html and attachments: still 78 characters a line, all ASCII.
        let long = "Überweisung ".repeat(12);
        let files = Value::Record(BTreeMap::from([("payout.csv".into(), text("id,amount\n7,1500\n")), ("Bericht ä.pdf".into(), Value::Bytes((0..=255).collect()))]));
        let m = message(&record(&[("to", text("a@acme.com")), ("subject", text(&long)), ("text", text(&format!("{} \n", "é".repeat(100)))), ("html", text("<p>Freigabe</p>")), ("attachments", files)]), Some("*")).unwrap();
        let sent = String::from_utf8(mime(&m, "Trésorerie <treasury@acme.com>", 0, "x@acme.com").unwrap()).unwrap();
        assert!(sent.is_ascii());
        assert!(sent.split("\r\n").all(|l| l.len() <= 78), "{sent}");
        assert!(!sent.replace("\r\n", "").contains('\n') && !sent.replace("\r\n", "").contains('\r'));
        for part in ["From: =?UTF-8?B?VHLDqXNvcmVyaWU=?= <treasury@acme.com>", "Subject: =?UTF-8?B?", "multipart/mixed; boundary=\"=_holon_mixed\"", "multipart/alternative", "Content-Type: text/csv; charset=utf-8; name=\"payout.csv\"", "filename*=UTF-8''Bericht%20%C3%A4.pdf", "aWQsYW1vdW50CjcsMTUwMAo=", "=C3=A9=", "=20\r\n", "--=_holon_mixed--\r\n"] {
            assert!(sent.contains(part), "{part} in {sent}");
        }
        assert_eq!(mime(&text("x"), "t@acme.com", 0, "x@acme.com"), None);
        assert_eq!(mime(&m, "not a sender", 0, "x@acme.com"), None);
    }

    #[test]
    fn what_isnt_a_message_it_may_send_is_nothing() {
        let base = [("to", text("a@acme.com")), ("subject", text("s")), ("text", text("t"))];
        let with = |k: &str, v: Value| {
            let mut fields: Vec<(&str, Value)> = base.iter().filter(|(n, _)| *n != k).cloned().collect();
            fields.push((k, v));
            record(&fields)
        };
        assert!(message(&record(&base), Some("a@acme.com")).is_some());
        assert_eq!(message(&record(&base), Some("b@acme.com")), None, "a recipient the text doesn't name");
        assert_eq!(message(&with("to", text("not one")), Some("*")), None);
        assert_eq!(message(&with("to", Value::List(vec![])), Some("*")), None, "no recipient");
        assert_eq!(message(&with("to", Value::List((0..51).map(|i| text(&format!("a{i}@acme.com"))).collect())), Some("*")), None, "past 50");
        assert_eq!(message(&with("subject", text("Hi\r\nBcc: everyone@acme.com")), Some("*")), None, "a header slipped in");
        assert_eq!(message(&with("subject", Value::Number(1.0)), Some("*")), None);
        assert_eq!(message(&with("bcc", text("a@acme.com")), Some("*")), None, "a field it doesn't know");
        assert_eq!(message(&with("attachments", record(&[("../x", text("t"))])), Some("*")), None, "a name outside");
        assert_eq!(message(&with("attachments", record(&[("x.json", record(&[]))])), Some("*")), None, "not a text or bytes");
        assert_eq!(message(&with("attachments", record(&[("big.bin", Value::Bytes(vec![0; MAX_MESSAGE_BYTES]))])), Some("*")), None, "past 10 MiB");
        assert_eq!(message(&text("a@acme.com"), Some("*")), None);
    }
}
