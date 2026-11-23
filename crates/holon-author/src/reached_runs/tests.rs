//! Every sample's sites, read off its runs beside what today's graph did there.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use holon_core::{parse_json, Unit, Value};

use super::*;
use crate::samples::samples;

/// The bodies each handler is given: golden's three (none, a record of the fields samples read,
/// a list).
fn bodies() -> Vec<String> {
    crate::golden::bodies()
}

fn record(pairs: &[(&str, Value)]) -> Value {
    Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// What a handler of `kind` is given on each of golden's bodies, the host's records in the shapes
/// the catalog documents (holon-core docs.rs): a route its request, an action and a websocket's
/// message the body as a value; a file's handler its file's handle, a mail's its message (the body
/// as its text), a feed's its event and a queue's its message (the body as their data); every
/// run its start time; state empty, as on a first run.
pub(crate) fn given(h: &Handler, unit: &Unit) -> Vec<Given> {
    bodies()
        .iter()
        .map(|b| {
            let value = parse_json(if b.is_empty() { "{}" } else { b }).unwrap_or(Value::Record(BTreeMap::new()));
            let mut sourced = BTreeMap::new();
            let input = match h.kind {
                "route" => vec![Some(request(&h.method, &h.path, b))],
                "action" | "websocket" => vec![Some(value.clone()); unit.inputs.len()],
                _ => vec![Some(value.clone()); unit.inputs.len()],
            };
            let n = |x: f64| Value::Number(x);
            // Every run has its start time, as the host supplies it; state is empty, as on a
            // deployment's first run.
            sourced.insert("clock".to_string(), n(1_763_856_000.0));
            match h.kind {
                "file" => {
                    let handle = record(&[("folder", text(&h.path)), ("name", text("a.csv")), ("size", n(b.len() as f64)), ("hash", text("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")), ("modified", n(1_763_856_000.0)), ("event", text("new"))]);
                    sourced.insert("file_arrival".to_string(), handle);
                }
                "mail" => {
                    let mail = record(&[
                        ("from", text("ada@example.com")),
                        ("name", text("Ada")),
                        ("to", Value::List(vec![text("ops@example.com")])),
                        ("cc", Value::List(vec![])),
                        ("subject", text("pay")),
                        ("date", text("2026-11-23T10:00:00Z")),
                        ("message_id", text("<1@example.com>")),
                        ("text", text(b)),
                        ("html", text("")),
                        ("attachments", Value::Record(BTreeMap::new())),
                        ("authenticated", Value::Boolean(true)),
                        ("verdict", text("pass")),
                    ]);
                    sourced.insert("mail_arrival".to_string(), mail);
                }
                "feed" => {
                    sourced.insert("feed_event".to_string(), record(&[("data", value.clone()), ("id", text("1")), ("event", text("message"))]));
                }
                "queue" => {
                    let message = record(&[("data", value.clone()), ("id", text("m1")), ("attributes", Value::Record(BTreeMap::new())), ("sent", n(1_763_856_000.0)), ("count", n(1.0)), ("group", text(""))]);
                    sourced.insert("queue_message".to_string(), message);
                }
                _ => {}
            }
            (input, sourced)
        })
        .collect()
}

/// The census of step C1: every sample's sites on every run, what their paths give beside what
/// today's graph did there. Watching the lowering changes nothing it gives; and no sample's site
/// is made where its path says it isn't reached, or missed where it is and has what it needs.
/// Printed with `--nocapture`: the count of each verdict, and each site run that isn't plainly
/// agreeing.
///
/// **If it fails:** a sample's site differs from its path. Read the line it names: either the
/// Reader's path for it is wrong (fix the path, in `lang/ts.rs`), or the graph does other than
/// JavaScript there, which is a new hole: a named row in `holon`'s `tests/reached.rs`, never a
/// patch here.
#[test]
fn every_samples_sites_are_read_off_its_runs() {
    let mut report = String::new();
    let mut differs = Vec::new();
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut sites_all = 0;
    for s in samples() {
        let w = Watched::of(s.text).unwrap();
        let plain = ts::compile(s.text).unwrap();
        assert!(plain.doc == w.lowered.doc, "{}: watching the lowering changed what it gives", s.name);
        sites_all += w.lowered.sites.len();
        for r in site_runs(&w, &given, answer_ok) {
            let site = &w.lowered.sites[r.site];
            let verdict = r.verdict(site);
            *tally.entry(format!("{verdict:?}")).or_default() += 1;
            if !matches!(verdict, Verdict::Agrees | Verdict::NotCompared | Verdict::NothingToGoOn) {
                let line = s.text.lines().nth(site.at.start.line as usize - 1).unwrap_or("").trim();
                let said = format!("{} | {} | input {} | {} at line {} | {verdict:?} {:?} values {:?} | {line}", s.name, r.handler, r.input, site.does, site.at.start.line, r.reached, r.has_values);
                let _ = writeln!(report, "  {said}");
                if matches!(verdict, Verdict::MadeUnreached | Verdict::MissedReached) {
                    differs.push(said);
                }
            }
        }
    }
    println!("sites: {sites_all}");
    for (cell, n) in &tally {
        println!("  {n:5} {cell}");
    }
    println!("not plainly agreeing:
{report}");
    assert!(differs.is_empty(), "sample sites that differ from their paths:
{}", differs.join("
"));
}
