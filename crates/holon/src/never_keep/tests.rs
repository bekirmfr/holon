use std::collections::BTreeMap;
use std::sync::Arc;

use holon_core::Capability;

use super::*;
use crate::recorder::{LogEntry, LoggedClaim, LoggedExchange, LoggedRead, LoggedRound, NodeTiming};
use crate::secrets::{Redactor, Resolved, MIN_SECRET_LEN};
use crate::sync::Mutex;
use crate::wire::PerformedEntry;

const KEY: &str = "cvc";

/// An entry with a value in every place [`LogEntry::values_mut`] hands out, and a text in every
/// place `texts_mut` does: each a slot a form is put in.
fn full_entry() -> LogEntry {
    let v = || SValue::Text("·".into());
    let t = || Some("·".to_string());
    LogEntry {
        method: t(),
        url: t(),
        request: Some(v()),
        sourced: vec![("StateRead(\"s\")".into(), v())],
        state_before: BTreeMap::from([("s".to_string(), v())]),
        outputs: vec![Some(v())],
        performed: vec![PerformedEntry { capability: "Log(\"info\")".into(), payload: v() }],
        progress: vec![PerformedEntry { capability: "Log(\"info\")".into(), payload: v() }],
        events: BTreeMap::from([("paid".to_string(), v())]),
        reads: vec![LoggedRead { path: vec![0], request: v(), answer: Some(v()), file: None, withheld: false }],
        timeline: vec![NodeTiming { path: vec![0], start_us: 0.0, end_us: 1.0, round: 0, outputs: Some(vec![Some(v())]), reason: t() }],
        rounds: vec![LoggedRound { exchanges: vec![LoggedExchange { path: vec![0], request: v(), response: v(), start_us: 0.0, end_us: 1.0 }] }],
        response_body: t(),
        error: t(),
        claims: vec![LoggedClaim { key: "·".into(), waited_ms: 0.0, overtaken: false, held_ms: None }],
        ..LogEntry::default()
    }
}

fn value_slots(e: &mut LogEntry) -> usize {
    let mut n = 0;
    e.values_mut(&mut |_| n += 1);
    n
}

fn put_value(e: &mut LogEntry, slot: usize, to: &SValue) {
    let mut n = 0;
    e.values_mut(&mut |v| {
        if n == slot {
            *v = to.clone();
        }
        n += 1;
    });
}

fn put_text(e: &mut LogEntry, slot: usize, to: &str) {
    if let Some(s) = e.texts_mut().nth(slot) {
        *s = to.to_string();
    }
}

/// The entry as the host writes it, for a flow that never keeps `cvc`: the run's own redaction
/// (`firing::redactor_with`), with no secret and no tainted key of its own.
fn redacted(mut e: LogEntry) -> LogEntry {
    let vault = Arc::new(Mutex::new(Resolved::default()));
    let redactor = crate::firing::redactor_with(&mut e, &[], Redactor::tokens(&BTreeMap::new()), &vault, &[], &[KEY.to_string()]);
    redactor.entry(&mut e);
    e
}

/// Each form a value can stand in under its key, as it is written with the value (`with(value)`,
/// the value as the run had it: a text, or a JSON literal / a query's raw text for a text form),
/// and as the record should hold it (the same with the mark in its place). Beside it, in the
/// same value, an amount that holds the short value's characters (`1737` for `737`) and a
/// reference that holds the value whole (`"1" + value`), which only a value as long as a secret
/// may be is removed from, by its text.
enum Form {
    Value(&'static str, fn(SValue, SValue) -> SValue),
    Text(&'static str, fn(&str, &str) -> String),
}

fn forms() -> Vec<Form> {
    vec![
        Form::Value("a record's field", |v, r| SValue::Record(vec![("amount".into(), SValue::Number(1737.0)), ("cvc".into(), v), ("ref".into(), r)])),
        Form::Value("a field deeper, its key in capitals", |v, r| {
            SValue::Record(vec![("card".into(), SValue::List(vec![SValue::Record(vec![("CVC".into(), v), ("amount".into(), SValue::Number(1737.0))])])), ("ref".into(), r)])
        }),
        Form::Value("a header's pair", |v, r| SValue::List(vec![SValue::List(vec![SValue::Text("cvc".into()), v]), SValue::List(vec![SValue::Text("x-ref".into()), r])])),
        Form::Value("an effect's payload", |v, r| SValue::Effect { sink: "state_write".into(), payload: Box::new(SValue::Record(vec![("cvc".into(), v), ("ref".into(), r)])) }),
        Form::Text("a JSON text", |v, r| format!(r#"{{"amount":1737,"cvc":{v},"ref":{r}}}"#)),
        Form::Text("a JSON text, nested", |v, r| format!(r#"{{"amount":1737,"card":{{"Cvc":{v},"exp":"12/30"}},"ref":{r}}}"#)),
        Form::Text("a JSON text, in a list", |v, r| format!(r#"[{{"cvc":{v}}},{{"amount":1737,"ref":{r}}}]"#)),
        Form::Text("a URL's query", |v, r| format!("/pay?amount=1737&cvc={v}&ref={r}#top")),
        Form::Text("a form's body", |v, r| format!("amount=1737&cvc={v}&ref={r}")),
        // As a host that echoes a request answers: the body as a string of its answer.
        Form::Text("a JSON text in a JSON text's string", |v, r| {
            let body = format!(r#"{{"amount":1737,"cvc":{v},"ref":{r}}}"#);
            format!(r#"{{"data":{},"json":{{"amount":1737}},"note":"a=1"}}"#, serde_json::to_string(&body).unwrap())
        }),
        Form::Text("a URL's query in a JSON text's string", |v, r| format!(r#"{{"amount":1737,"url":{}}}"#,serde_json::to_string(&format!("https://h/pay?amount=1737&cvc={v}&ref={r}")).unwrap())),
    ]
}

/// The value under test, of `len` characters: digits, so a short one is in an amount (`737` in
/// `1737`).
fn canary(len: usize) -> String {
    "73".repeat(len).chars().take(len).collect()
}

/// **Guards:** a value under a key the flow never keeps is replaced where it stands, by
/// `‹never_keep:cvc›`, at every length (1 to 40), in every form a run's entry holds it in (a
/// record's field at depth, its key either case; a header's pair; an effect's payload; a JSON
/// text flat, nested, in a list, as a text, a number, a record; a query; a form; a JSON text or
/// a query inside a JSON text's string, as a host that echoes a request answers), in every place
/// of the entry ([`LogEntry::values_mut`], `texts_mut`): the entry is then the one it was with
/// the mark there and nothing else changed (the amount `1737` beside a CVC `737` stays). A value
/// as long as a secret may be is removed by its text elsewhere too; a shorter one isn't. A line
/// is redacted the same.
/// **Why:** a short value can't be removed by its text (it is in other values), so it is
/// guarded by its place, and a JSON text is spliced, not written again.
/// **If it fails:** a CVC is in the log, or the record differs from what the run had beyond the
/// CVC's place. A new place in the entry is covered here without anyone naming it: it is one of
/// `values_mut`'s.
#[test]
fn a_value_never_kept_is_replaced_where_it_stands_in_every_form_and_place_at_any_length() {
    let mark = mark(KEY);
    let slots = value_slots(&mut full_entry());
    let texts = full_entry().texts_mut().count();
    assert!(slots >= 12 && texts >= 6, "{slots} {texts}");
    let mut cases = 0;
    for len in [1, 3, 5, MIN_SECRET_LEN, 40] {
        let c = canary(len);
        let long = len >= MIN_SECRET_LEN;
        let reference = format!("1{c}");
        let reference_kept = if long { format!("1{mark}") } else { reference.clone() };
        for form in forms() {
            // As a text, and where the form can hold one, as a number and as a record.
            // (A number is that value only where it is written as it: not one of 40 digits.)
            let n: f64 = c.parse().unwrap();
            let number = (format!("{n}") == c).then(|| (SValue::Number(n), c.clone()));
            let as_values: Vec<(SValue, String)> = [Some((SValue::Text(c.clone()), serde_json::to_string(&c).unwrap())), number, Some((SValue::Record(vec![("n".into(), SValue::Text(c.clone()))]), format!(r#"{{"n":"{c}"}}"#)))].into_iter().flatten().collect();
            match form {
                Form::Value(name, make) => {
                    for (value, _) in &as_values {
                        let had = make(value.clone(), SValue::Text(reference.clone()));
                        let kept = make(SValue::Text(mark.clone()), SValue::Text(reference_kept.clone()));
                        for slot in 0..slots {
                            let mut e = full_entry();
                            put_value(&mut e, slot, &had);
                            let mut expected = full_entry();
                            put_value(&mut expected, slot, &kept);
                            let got = redacted(e);
                            assert_eq!(serde_json::to_value(&got).unwrap(), serde_json::to_value(&expected).unwrap(), "{name}, {len} characters, {value:?}, value slot {slot}");
                            cases += 1;
                        }
                    }
                }
                Form::Text(name, make) => {
                    let query = name.contains("query") || name.contains("form");
                    // A query holds a text as it is; JSON a text, a number or a record.
                    let values: Vec<String> = if query { vec![c.clone()] } else { as_values.iter().map(|(_, lit)| lit.clone()).collect() };
                    let quote = |s: &str| if query { s.to_string() } else { serde_json::to_string(s).unwrap() };
                    for lit in values {
                        let had = make(&lit, &quote(&reference));
                        let kept = make(&quote(&mark), &quote(&reference_kept));
                        for slot in 0..slots + texts {
                            let (mut e, mut expected) = (full_entry(), full_entry());
                            if slot < slots {
                                put_value(&mut e, slot, &SValue::Text(had.clone()));
                                put_value(&mut expected, slot, &SValue::Text(kept.clone()));
                            } else {
                                put_text(&mut e, slot - slots, &had);
                                put_text(&mut expected, slot - slots, &kept);
                            }
                            let got = redacted(e);
                            assert_eq!(serde_json::to_value(&got).unwrap(), serde_json::to_value(&expected).unwrap(), "{name}, {len} characters, {lit}, slot {slot}");
                            cases += 1;
                        }
                        // A line, as the log writes it (`firing::push_run`).
                        let vault = Arc::new(Mutex::new(Resolved::default()));
                        let lines = vec![("info".to_string(), holon_core::Value::Text(had.clone()))];
                        let r = crate::firing::redactor_with(&mut LogEntry::default(), &lines, Redactor::tokens(&BTreeMap::new()), &vault, &[], &[KEY.to_string()]);
                        assert_eq!(r.text(&had), kept, "a line: {name}, {len} characters, {lit}");
                    }
                }
            }
        }
    }
    assert!(cases > 1000, "{cases}");
}

/// **Guards:** whatever spelling of a key a flow can read as that key, the value under it is
/// replaced, in every place the record keeps a request: the key written plainly, with JSON's
/// `\u` escapes (in part or whole), percent-encoded in a query or a form (in part or whole, beside
/// a `+`), in another case, a JSON text nested in a JSON string, a `?` in a form's value; as the
/// raw body, a URL, the request the host builds (its parsed query, its headers), the value
/// `JSON.parse` gives, and inside a JSON string. Each row is first held to the flow: the host's
/// own reading (`data::request_record`, core's JSON parser) finds the key there. Then the CVC is
/// nowhere in the entry and its mark is.
/// **Why:** the caller chooses the spelling: a shortcut that passed by a text whose raw bytes
/// don't name the key kept `{"\u0063vc":"739"}` and `%63vc=739` whole, though the flow read both
/// as `cvc`.
/// **If it fails:** the row names a spelling and a place the record keeps the CVC in: the key is
/// compared before it is decoded there (`replace_text`'s shortcut, `spelled_as_read`, the query's
/// pairs), or that text isn't read as what it is.
#[test]
fn whatever_spelling_a_flow_reads_as_the_key_is_replaced_wherever_the_request_is_kept() {
    const CVC: &str = "739";
    // (the spelling, JSON (true) or a query or a form, the text)
    let rows: Vec<(&str, bool, String)> = vec![
        ("plain", true, r#"{"cvc":"739","amount":1250}"#.into()),
        (r"a \u escape", true, r#"{"\u0063vc":"739","amount":1250}"#.into()),
        (r"all \u escapes", true, r#"{"amount":1250,"\u0063\u0076\u0063":"739"}"#.into()),
        ("a number", true, r#"{"\u0063vc":739}"#.into()),
        ("mixed case", true, r#"{"CvC":"739"}"#.into()),
        (r"a \/ and an escape elsewhere", true, r#"{"url":"a\/b","cvc":"739"}"#.into()),
        ("nested in a JSON string", true, r#"{"data":"{\"\\u0063vc\":\"739\"}"}"#.into()),
        ("a query, plain", false, "cvc=739&amount=1250".into()),
        ("percent-encoded", false, "amount=1250&%63vc=739".into()),
        ("percent-encoded whole", false, "%63%76%63=739".into()),
        ("percent-encoded, a + beside", false, "note=a+b&c%76c=739".into()),
        ("mixed case", false, "CVC=739".into()),
        ("a ? in a later value", false, "cvc=739&next=?x".into()),
        ("a fragment after", false, "x=1&%43vc=739#top".into()),
    ];
    // Whether the flow reads `key` from what `v` holds (either case, as never_keep compares), a
    // JSON text in a string read again.
    fn reads(v: &holon_core::Value) -> bool {
        match v {
            holon_core::Value::Record(fields) => fields.iter().any(|(k, x)| k.eq_ignore_ascii_case(KEY) || reads(x)),
            holon_core::Value::List(xs) => xs.iter().any(reads),
            holon_core::Value::Text(t) => holon_core::parse_json(t).is_some_and(|v| reads(&v)),
            _ => false,
        }
    }
    let headers = vec![("CVC".to_string(), CVC.to_string())];
    let mut cases = 0;
    for (what, json, text) in &rows {
        // The host's own reading of it, as a body and as a query.
        let as_query = crate::data::request_record("POST", "/pay", BTreeMap::new(), text, &headers, "");
        let as_body = crate::data::request_record("POST", "/pay", BTreeMap::new(), "", &headers, text);
        // (Without the header, which names the key whatever the row.)
        let flow_reads = match json {
            true => holon_core::parse_json(text).is_some_and(|v| reads(&v)),
            false => reads(&crate::data::request_record("POST", "/pay", BTreeMap::new(), text, &[], "")),
        };
        assert!(flow_reads, "{what}: the flow doesn't read {text} as {KEY}: not a row");
        let mut places: Vec<(&str, SValue)> = vec![
            ("the raw body", SValue::Record(vec![("body".into(), SValue::Text(text.clone()))])),
            ("the request the host builds of a body", SValue::from_core(&as_body)),
            ("inside a JSON string", SValue::Text(serde_json::to_string(&serde_json::json!({ "data": text })).unwrap())),
        ];
        match json {
            true => places.push(("the value JSON.parse gives", SValue::from_core(&holon_core::parse_json(text).unwrap()))),
            false => {
                places.push(("a URL", SValue::Record(vec![("url".into(), SValue::Text(format!("/pay?{text}")))])));
                places.push(("the request the host builds of a query", SValue::from_core(&as_query)));
            }
        }
        for (place, v) in places {
            let mut e = LogEntry { request: Some(v.clone()), outputs: vec![Some(v)], ..LogEntry::default() };
            if !json {
                e.url = Some(format!("/pay?{text}"));
            }
            let got = serde_json::to_string(&redacted(e)).unwrap();
            assert!(!got.contains(CVC) && got.contains(&mark(KEY)), "{what}, {place}: {text} is kept as {got}");
            cases += 1;
        }
    }
    assert!(cases >= 60, "{cases}");
}

/// **Guards:** a text a run received that reads as the mark of a value never kept is written
/// escaped, as a look-alike of any mark is, and reads back as the run had it; the log's own mark
/// reads back as a mark.
/// **Why:** `holon verify` and replay trust only the log's own marks: a caller who sends
/// `‹never_keep:cvc›` must not pass for one.
/// **If it fails:** `never_keep:` is missing from the kinds of mark the log knows (`MARKED`).
#[test]
fn a_look_alike_of_the_mark_is_escaped_and_the_marks_own_read_as_marks() {
    let mut e = full_entry();
    put_value(&mut e, 0, &SValue::Record(vec![("cvc".into(), SValue::Text("737".into())), ("note".into(), SValue::Text(mark(KEY)))]));
    let got = redacted(e);
    assert!(got.escaped);
    let SValue::Record(fields) = got.request.unwrap() else { panic!() };
    let read = |v: &SValue| match v {
        SValue::Text(s) => crate::secrets::read_marked(s, true),
        _ => panic!(),
    };
    assert_eq!(read(&fields[0].1), [(String::new(), false), (mark(KEY), true), (String::new(), false)]);
    assert_eq!(read(&fields[1].1), [(mark(KEY), false)]);
}

/// Every capability a leaf's effect has (what `taint.rs` calls a sink: one not sourced), from
/// every leaf core knows (`GENESIS`), by its kind's name.
fn sink_kinds() -> BTreeMap<String, Capability> {
    let mut kinds = BTreeMap::new();
    for &name in holon_core::GENESIS {
        let Some(unit) = holon_core::interface_of(name) else { continue };
        for cap in holon_core::capabilities_of(name, &unit.params).into_iter().flat_map(|c| c.iter().collect::<Vec<_>>()) {
            if !cap.is_sourced() {
                let kind = format!("{cap:?}");
                kinds.insert(kind.split('(').next().unwrap_or(&kind).to_string(), cap);
            }
        }
    }
    kinds
}

/// **Guards:** every kind of effect a leaf has is either one that keeps or hands on what it is
/// given (refused when that holds a value never kept, under its key, in any form a record holds
/// it in; performed when it holds none) or named here with why not; a request to a host is
/// performed with it. And a run's writes, lines and calls are held to it (`firing.rs`,
/// `calls.rs`): see `a_flow_never_keeps_what_its_writes_would_keep` in tests.rs.
/// **Why:** a CVC written to state, a collection, a file, a database, a channel, a mail, a queue,
/// the log, or handed to another flow, is kept; sending it to the processor is the point.
/// **If it fails:** a new kind of effect isn't classified: add it to [`keeps`], or here with why
/// what it is given isn't kept.
#[test]
fn every_effect_that_keeps_what_it_is_given_refuses_a_value_never_kept() {
    let not_kept: BTreeMap<&str, &str> = BTreeMap::from([
        ("HttpRequest", "sent to its host: the point of taking a card's CVC; its record is redacted"),
        ("NetworkEgress", "the same request, as a kind"),
        ("FileServe", "a route's answer, never performed"),
        ("ChannelRead", "a subscription, a route's answer, never performed"),
        ("Spawn", "no leaf starts a process"),
    ]);
    let keys = vec![KEY.to_string()];
    let holding = [
        holon_core::parse_json(r#"{"amount":1737,"cvc":"737"}"#).unwrap(),
        holon_core::parse_json(r#"{"card":{"CVC":7}}"#).unwrap(),
        holon_core::Value::List(vec![holon_core::Value::List(vec![holon_core::Value::Text("cvc".into()), holon_core::Value::Text("1".into())])]),
        holon_core::Value::Text(r#"{"items":[{"cvc":"737"}]}"#.into()),
        holon_core::Value::Text("amount=1737&cvc=737".into()),
        holon_core::Value::Text(r#"{"data":"{\"cvc\":\"737\"}","note":"cvc=737"}"#.into()),
    ];
    let clean = [holon_core::parse_json(r#"{"amount":1737,"code":"737"}"#).unwrap(), holon_core::Value::Text("cvc 737".into()), holon_core::Value::Text(r#"{"note":"the cvc is 737"}"#.into())];
    let kinds = sink_kinds();
    assert!(kinds.contains_key("StateWrite") && kinds.contains_key("HttpRequest") && kinds.contains_key("RunsAction"), "{kinds:?}");
    for (kind, cap) in &kinds {
        let keeps = keeps(cap);
        assert!(keeps != not_kept.contains_key(kind.as_str()), "{kind}: {}", if keeps { "kept and named as not" } else { "neither kept nor named" });
        for payload in &holding {
            let why = refused(cap, payload, &keys);
            assert_eq!(why.is_some(), keeps, "{kind} given {payload:?}");
            if let Some(why) = why {
                assert!(why.contains("\"cvc\"") && why.contains("never keeps") && !why.contains("737"), "{why}");
            }
        }
        for payload in &clean {
            assert_eq!(refused(cap, payload, &keys), None, "{kind} given {payload:?}");
        }
        assert_eq!(refused(cap, &holding[0], &[]), None, "{kind}: a flow that never keeps no key");
    }
    for kind in not_kept.keys() {
        assert!(kinds.contains_key(*kind) || *kind == "Spawn", "{kind} is no leaf's: take it out");
    }
}
