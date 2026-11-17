//! The keys a flow never keeps (`never_keep: ["cvc"]`, `holon_core::Server::never_keep`), guarded
//! by where a value stands: under its key. A run's record has each such value replaced where it
//! stands, whatever its length, by `‹never_keep:cvc›` ([`Places`], applied by the redactor that
//! writes the record, `secrets::Redactor`), and a write or a send that holds one under its key is
//! refused ([`held`], `firing.rs`, `calls.rs`).
//!
//! By place, not by text: a tainted value is found once and then removed by its text everywhere,
//! which a short value can't be (a CVC `737` inside an amount `1737`), so a tainted value shorter
//! than `secrets::MIN_SECRET_LEN` is left as it is. A key never kept is replaced where it stands,
//! at any length, and nothing else is touched; a value of that length or more is removed by its
//! text too, as a tainted one is (`data::never_kept`).
//!
//! The places are those `data::tainted` finds a tainted key's value at: a record's field at any
//! depth, a header's `[name, value]` pair, a JSON text's field at any depth (spliced over the
//! value's own bytes, every other byte of the text as it was), and a query's or a form's
//! `name=value`, each of these two inside a JSON text's string too (a host that echoes a request's
//! body answers with it as a string: that string alone is written again). Names match either case. A value read out of its key into a plain value
//! (`const c = req.json.cvc`) stands under no key: following it is the next stage, not this one.

use crate::wire::SValue;

/// How the mark of a value never kept begins: `‹never_keep:cvc›`.
pub(crate) const MARK: &str = "‹never_keep:";

/// The mark the record writes where a value of `key` stood.
pub(crate) fn mark(key: &str) -> String {
    format!("{MARK}{key}›")
}

/// The key of `keys` that `name` is, either case: as the flow wrote it.
fn key_of<'k>(keys: &'k [String], name: &str) -> Option<&'k str> {
    keys.iter().map(String::as_str).find(|k| k.eq_ignore_ascii_case(name))
}

/// Whether `s` names any of `keys`, either case, as it is written: a text that names none is passed
/// by unread, but only where how it is written is how it reads ([`spelled_as_read`]).
fn names_any(s: &str, keys: &[String]) -> bool {
    keys.iter().any(|k| s.as_bytes().windows(k.len()).any(|w| w.eq_ignore_ascii_case(k.as_bytes())))
}

/// Each value in `v` that stands under one of `keys`, replaced whole by what `put` makes of its key
/// (as the flow wrote it); the keys replaced, in the order met.
pub(crate) fn replace(v: &mut SValue, keys: &[String], put: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut hits = Vec::new();
    walk(v, keys, put, &mut hits);
    hits
}

fn walk(v: &mut SValue, keys: &[String], put: &dyn Fn(&str) -> String, hits: &mut Vec<String>) {
    match v {
        SValue::Record(fields) => {
            for (name, x) in fields.iter_mut() {
                match key_of(keys, name) {
                    Some(key) => {
                        *x = SValue::Text(put(key));
                        hits.push(key.to_string());
                    }
                    None => walk(x, keys, put, hits),
                }
            }
        }
        SValue::List(xs) => {
            // A header as a pair, `[name, value]`.
            if let [SValue::Text(name), x] = xs.as_mut_slice()
                && let Some(key) = key_of(keys, name)
            {
                *x = SValue::Text(put(key));
                hits.push(key.to_string());
                return;
            }
            xs.iter_mut().for_each(|x| walk(x, keys, put, hits));
        }
        SValue::Text(s) => hits.extend(replace_text(s, keys, put)),
        SValue::Effect { payload, .. } | SValue::Sum { payload, .. } => walk(payload, keys, put, hits),
        SValue::Number(_) | SValue::Boolean(_) | SValue::Bytes(_) => {}
    }
}

/// Each value in the text `s` that stands under one of `keys`, replaced in place: a JSON text's
/// (a body, a call's answer, a text the flow built) by `put`'s text as a JSON string over the
/// value's own bytes, the rest of the text as it was; a query's or a form's (`a=1&cvc=737`, after a
/// URL's `?`) by `put`'s text over the value. The keys replaced.
pub(crate) fn replace_text(s: &mut String, keys: &[String], put: &dyn Fn(&str) -> String) -> Vec<String> {
    let t = s.trim_start();
    let json = t.starts_with('{') || t.starts_with('[');
    if !(json || s.contains('=')) || (spelled_as_read(s) && !names_any(s, keys)) {
        return Vec::new();
    }
    // Each span (`[start, end)`) with what it is replaced by and the keys that made it, in the
    // text's order.
    let mut spans: Vec<Span> = Vec::new();
    // JSON as the flow reads it (`JSON.parse`, `req.json`: core's parser), or as serde does: a
    // text either reads is scanned, so one only the flow's parser takes isn't read as a query.
    if json && (holon_core::parse_json(s).is_some() || serde_json::from_str::<serde_json::Value>(t).is_ok()) {
        Scan { s, i: 0, keys, put }.value(true, &mut spans);
    } else {
        // A query or a form: its pairs read from the whole text (a form's body) and from after its
        // first `?` (a URL's query, as the host splits it), each name percent-decoded, `+` a
        // space, as the host decodes `req.query`. Both readings, so a `?` in a form's value, or an
        // `=` in a URL's path, hides no pair; where they overlap, the span covers both.
        let q = s.find('?');
        let end = q.and_then(|q| s[q..].find('#').map(|h| q + h)).unwrap_or(s.len());
        for start in [0].into_iter().chain(q.map(|q| q + 1)) {
            let mut at = start;
            for pair in s[start..end.max(start)].split('&') {
                if let Some(eq) = pair.find('=') {
                    let name = &pair[..eq];
                    let name = name.rsplit_once('?').map_or(name, |(_, after)| after);
                    if let Some(key) = key_of(keys, crate::data::percent_decode(name.trim()).as_str()) {
                        spans.push((at + eq + 1, at + pair.len(), put(key), vec![key.to_string()]));
                    }
                }
                at += pair.len() + 1;
            }
        }
        spans.sort_by_key(|(a, b, ..)| (*a, std::cmp::Reverse(*b)));
        let mut merged: Vec<Span> = Vec::new();
        for span in spans {
            match merged.last_mut() {
                Some(last) if span.0 < last.1 => last.1 = last.1.max(span.1),
                _ => merged.push(span),
            }
        }
        spans = merged;
    }
    // From the last, so the earlier ones stay where they are.
    for (a, b, with, _) in spans.iter().rev() {
        s.replace_range(*a..*b, with);
    }
    spans.into_iter().flat_map(|(_, _, _, keys)| keys).collect()
}

/// Whether every name in `s` reads as it is written: no `\` (a JSON escape, `"\u0063vc"` read as
/// `cvc`), no `%` (a query's or a form's escape, `%63vc`) and no `+` (a space there). Only then
/// does a text that doesn't name a key as written name none as read (`names_any`, either case,
/// is a superset of the flow's exact `req.json.cvc` and `req.query.cvc`), and is passed by
/// unread; any other is read whole, its names decoded first.
fn spelled_as_read(s: &str) -> bool {
    !s.contains(['\\', '%', '+'])
}

/// A span of a text (`[start, end)`), what it is replaced by, and the keys that made it.
type Span = (usize, usize, String, Vec<String>);

/// The first of `keys` that `v` holds a value under, by place: what a write or a send that would
/// keep it is refused for. None: it holds none.
pub(crate) fn held(v: &holon_core::Value, keys: &[String]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let mut v = SValue::from_core(v);
    replace(&mut v, keys, &|_| String::new()).into_iter().next()
}

/// Whether the text `s` holds a value under one of `keys`, by place (a JSON text's field, a
/// query's or a form's `name=value`, either inside a JSON text's string): what a run computed from
/// a secret is listed in its record by a keyed hash (`secrets::born`), and such a text's hash
/// would be the value a few tries away (a CVC is a thousand), so none is listed for it.
pub(crate) fn in_text(s: &str, keys: &[String]) -> bool {
    !keys.is_empty() && !replace_text(&mut s.to_string(), keys, &|_| String::new()).is_empty()
}

/// Whether what an effect of `cap` is given is kept, or handed on to what keeps it: a state, a
/// collection, a file, a database, a channel, a mail, a queue, the flow's log, another flow's
/// action. A request to a host is not (sending a card's CVC to the payment processor is the
/// point of taking it; its logged form is redacted as the rest of the record is), nor is what
/// reads (a subscription, a served file: a route's answers, never performed).
pub(crate) fn keeps(cap: &holon_core::Capability) -> bool {
    use holon_core::Capability::*;
    matches!(cap, StateWrite(_) | CollectionWrite(_) | DbWrite(_) | FileWrite(_) | ChannelSend(_) | MailSend(_) | QueueSend(_) | Log(_) | RunsAction(_))
}

/// Why an effect of `cap` given `payload` is refused: it keeps what it is given ([`keeps`]), and
/// that holds a value under one of `keys` ([`held`]). Names the key, never the value.
pub(crate) fn refused(cap: &holon_core::Capability, payload: &holon_core::Value, keys: &[String]) -> Option<String> {
    if !keeps(cap) {
        return None;
    }
    held(payload, keys).map(|key| format!("refused {cap:?}: it holds {key:?}, which this flow never keeps"))
}

/// A JSON text read for the spans of the values under the keys sought: well formed already (it
/// parsed), so it is read without checking. A member under a key is replaced by `put`'s text as a
/// JSON string; a string under none that holds a text with such a value (a JSON text or a query
/// inside a string, as a host that echoes a request's body answers it) is replaced by itself with
/// that value replaced ([`replace_text`]), written again as a string.
struct Scan<'a> {
    s: &'a str,
    i: usize,
    keys: &'a [String],
    put: &'a dyn Fn(&str) -> String,
}

impl Scan<'_> {
    fn byte(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn ws(&mut self) {
        while self.byte().is_some_and(|b| b.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    /// A string, decoded, the scan past its closing quote.
    fn string(&mut self) -> String {
        let start = self.i;
        self.i += 1;
        while let Some(b) = self.byte() {
            self.i += 1;
            match b {
                b'\\' => self.i += 1,
                b'"' => break,
                _ => {}
            }
        }
        serde_json::from_str(&self.s[start..self.i]).unwrap_or_default()
    }

    /// A value: each member under a key sought is a span, and not looked into; any other is
    /// looked into, a string too. `seek`: false inside a member already replaced.
    fn value(&mut self, seek: bool, spans: &mut Vec<Span>) {
        self.ws();
        match self.byte() {
            Some(b'{') => {
                self.i += 1;
                loop {
                    self.ws();
                    match self.byte() {
                        Some(b'}') => {
                            self.i += 1;
                            return;
                        }
                        Some(b',') => {
                            self.i += 1;
                            continue;
                        }
                        Some(b'"') => {}
                        _ => return,
                    }
                    let name = self.string();
                    self.ws();
                    self.i += 1; // the `:`
                    self.ws();
                    let start = self.i;
                    match key_of(self.keys, &name).filter(|_| seek) {
                        Some(key) => {
                            self.value(false, spans);
                            let with = serde_json::to_string(&(self.put)(key)).expect("a text");
                            spans.push((start, self.i, with, vec![key.to_string()]));
                        }
                        None => self.value(seek, spans),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                loop {
                    self.ws();
                    match self.byte() {
                        Some(b']') => {
                            self.i += 1;
                            return;
                        }
                        Some(b',') => self.i += 1,
                        None => return,
                        _ => self.value(seek, spans),
                    }
                }
            }
            Some(b'"') => {
                let start = self.i;
                let mut text = self.string();
                if seek {
                    let found = replace_text(&mut text, self.keys, self.put);
                    if !found.is_empty() {
                        spans.push((start, self.i, serde_json::to_string(&text).expect("a text"), found));
                    }
                }
            }
            Some(_) => {
                while self.byte().is_some_and(|b| !matches!(b, b',' | b']' | b'}') && !b.is_ascii_whitespace()) {
                    self.i += 1;
                }
            }
            None => {}
        }
    }
}

/// A run's keys never kept, with what stands in for each value while its record is redacted: a
/// text no run can write (characters of Unicode's private use, around a fresh random nonce), which
/// the redactor then writes as the key's mark, as a mark of its own (never escaped as a run's
/// look-alike).
pub(crate) struct Places {
    keys: Vec<String>,
    nonce: String,
}

impl Places {
    /// None: the flow never keeps no key.
    pub(crate) fn new(keys: &[String]) -> Option<Places> {
        if keys.is_empty() {
            return None;
        }
        let mut bytes = [0u8; 12];
        getrandom::fill(&mut bytes).expect("the OS random source");
        Some(Places { keys: keys.to_vec(), nonce: bytes.iter().map(|b| format!("{b:02x}")).collect() })
    }

    /// What stands in for the value of the `i`th key.
    fn placeholder(&self, i: usize) -> String {
        format!("\u{F8F0}{}.{i}\u{F8F1}", self.nonce)
    }

    fn put(&self, key: &str) -> String {
        self.placeholder(self.keys.iter().position(|k| k == key).expect("one of its keys"))
    }

    /// Each stand-in with its key's mark: what the redactor replaces it by. One pair a stand-in,
    /// not one a spelling (`secrets::spellings`): its characters are none JSON escapes (no quote,
    /// backslash or control character), so written out as JSON at any depth it reads the same.
    pub(crate) fn pairs(&self) -> Vec<(String, String)> {
        self.keys.iter().enumerate().map(|(i, k)| (self.placeholder(i), mark(k))).collect()
    }

    /// `v` with each value under a key stood in for.
    pub(crate) fn svalue(&self, v: &mut SValue) {
        replace(v, &self.keys, &|k| self.put(k));
    }

    /// `s` with each value under a key stood in for.
    pub(crate) fn text(&self, s: &mut String) {
        replace_text(s, &self.keys, &|k| self.put(k));
    }

    /// Everything of `e` a run could have put a value into ([`LogEntry::values_mut`], its texts),
    /// each value under a key stood in for.
    pub(crate) fn entry(&self, e: &mut crate::recorder::LogEntry) {
        e.values_mut(&mut |v| self.svalue(v));
        for s in e.texts_mut() {
            self.text(s);
        }
    }
}

#[cfg(test)]
mod tests;
