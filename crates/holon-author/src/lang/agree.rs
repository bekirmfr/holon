//! The text and the canvas agree: a text is taken only if the steps it lowers to can be written
//! back as text ([`print`]) and that text lowers to the same steps again.
//!
//! The way in ([`ts`] and [`lower`]) and the way back ([`print`]) are written by hand, each on its
//! own; nothing else holds them to each other. A text the way in takes and the way back can't
//! write would deploy as steps nobody can open on the canvas or read as text again, and in the
//! cases found that way the steps also don't do what the text says (an action started inside a
//! function's `if` starts though the `if` is false; a write in a list's function is dropped).
//! So [`lang::compile`], the one door every reading of a text passes (the editor's mistakes,
//! `holon check` and `deploy`, an agent's plan, the server's check that a version's text is the
//! code it runs), refuses it, in the words of the one who wrote the text, at the function the
//! way back stopped at, else at the `flow(…)` line.
//!
//! The round trip reads the printed text with [`ts::compile`], not [`lang::compile`]: the printer
//! itself compiles what it wrote to check its actions ([`print`]'s `same_actions`, with
//! [`ts::compile`] too), so neither ever comes back here.
//!
//! [`lang::compile`]: crate::lang::compile
//! [`lower`]: crate::lang::lower

use std::collections::BTreeMap;

use holon_core::MapResolver;

use crate::composites::Names;
use crate::flow::flow_unit;
use crate::lang::ast::{HeaderKind, Item, Pos, Span};
use crate::lang::lower::{LowerError, Lowered};
use crate::lang::{print, ts};
use crate::text::restore;
use crate::to_hex;

/// Where a mistake with no place of its own is said: the text's start.
const START: Span = Span { start: Pos { line: 1, col: 1 }, end: Pos { line: 1, col: 1 } };

/// `lowered`, the text `src` read, refused unless it is written back as a text that reads as the
/// same flow unit (and so the same units under it: a unit's hash is that of all it is made of).
pub(crate) fn written_back(src: &str, lowered: &Lowered) -> Result<(), LowerError> {
    let mut store = MapResolver::new();
    let text = match print::print(lowered.name.as_deref(), &lowered.doc, &mut store, &Names::new(), &lowered.names) {
        Ok(text) => text,
        // A flow wrong as a whole (two routes on one path, a flow that doesn't build) is refused
        // where it is checked whole, in its own words, which the printer's would only hide.
        Err(_) if !whole(&lowered.doc) => return Ok(()),
        Err(e) => return Err(refusal(src, lowered, &e)),
    };
    let back = ts::compile(&text).map_err(|e| LowerError {
        span: flow_span(src),
        message: format!("written back from its steps, this text doesn't read again ({e}), so the canvas couldn't show it: write it another way (and please report it: Holon's two readings of a text should agree)"),
    })?;
    // A document whose flow doesn't build is refused where it is checked whole, with its own
    // reason; only the units of one that builds are compared.
    if let Ok(was) = unit_of(&lowered.doc)
        && unit_of(&back.doc).ok().as_ref() != Some(&was)
    {
        return Err(LowerError {
            span: flow_span(src),
            message: "written back from its steps, this text reads as another flow, so the canvas would show something other than what runs: write it another way (and please report it: Holon's two readings of a text should agree)".into(),
        });
    }
    Ok(())
}

/// Whether `doc` is a flow with no mistake of its own as a whole: its handlers derive with no
/// error ([`derive_server`]) and its unit builds. One that has one is refused by those who check
/// the flow whole (`holon check`, an agent's plan), saying it.
///
/// [`derive_server`]: crate::flow::derive_server
fn whole(doc: &crate::builder::FlowDocument) -> bool {
    let mut store = MapResolver::new();
    restore(doc, &mut store, &Names::new());
    let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
    crate::flow::derive_server(&doc.nodes, &doc.edges, &mut store, port, doc.unit_settings()).errors.is_empty() && unit_of(doc).is_ok()
}

/// The flow unit `doc` deploys as, in hex.
fn unit_of(doc: &crate::builder::FlowDocument) -> Result<String, String> {
    let mut store = MapResolver::new();
    restore(doc, &mut store, &Names::new());
    let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
    flow_unit(&doc.nodes, &doc.edges, &mut store, port, doc.unit_settings()).map(|h| to_hex(&h))
}

/// Why the steps of `src` can't be written back (`said`, the printer's words for the canvas), in
/// the words of the one who wrote the text, at the function it names when it names one.
fn refusal(src: &str, lowered: &Lowered, said: &str) -> LowerError {
    // The printer's own text not reading again: its reason, at a place in that text, not this.
    if let Some(why) = said.strip_prefix("the TypeScript written for this flow doesn't read back (").and_then(|r| r.split_once("), which is the printer's mistake")).map(|(why, _)| why) {
        let why = why.split_once(": ").filter(|(at, _)| at.split(':').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))).map_or(why, |(_, why)| why);
        return LowerError { span: flow_span(src), message: format!("written back as text from its steps, this flow doesn't read again: {why}") };
    }
    let named = said.strip_prefix("the function \"").and_then(|rest| rest.split_once('"'));
    let Some((name, rest)) = named else {
        return LowerError { span: flow_span(src), message: format!("this flow's steps can't be written back as text, so the canvas couldn't show it: {said}") };
    };
    let (what, span, caller) = match placed_in(lowered, name) {
        Some((leaf, span)) => (format!("the function given to {leaf}"), span, "move it out of the function, into the route or action itself".to_string()),
        None if anonymous(name) => ("a function written in place".to_string(), flow_span(src), "move it out of the function, into the route or action itself".to_string()),
        None => (format!("the function \"{name}\""), function_span(src, name).unwrap_or_else(|| flow_span(src)), format!("move it into the route or action that calls \"{name}\"")),
    };
    let message = if rest.contains("tied to nothing") {
        format!("{what} has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: {caller}")
    } else if let Some(doing) = rest.strip_prefix(" computes: ").and_then(|r| r.split_once(" another flow's action")).map(|(doing, _)| doing) {
        let (does, verb) = if doing == "calling" { ("calls", "call") } else { ("starts", "start") };
        let where_ = match anonymous(name) {
            true => "in the route or action itself".to_string(),
            false => format!("in the route or action that calls \"{name}\""),
        };
        format!("{what} {does} another flow's action: a function only computes, so {verb} it {where_}")
    } else if rest.starts_with(" computes: ") {
        let reason = rest.trim_start_matches(" computes: ");
        format!("{what} doesn't only compute ({reason}): a function only computes, so {caller}")
    } else {
        format!("{what} can't be written back as text, so the canvas couldn't show it:{rest}")
    };
    LowerError { span, message }
}

/// Whether `name` is one the reader made up for a function written in place (`fn_1`, and the
/// printer's `fn_1_` when that is taken).
fn anonymous(name: &str) -> bool {
    name.strip_prefix("fn_").map(|k| k.trim_end_matches('_')).is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()))
}

/// For a function written in place, `name` (`fn_1`): the leaf it is given to (`map`) and that
/// leaf's place in the text, when the leaf is a handler's own step.
fn placed_in(lowered: &Lowered, name: &str) -> Option<(String, Span)> {
    if !anonymous(name) {
        return None;
    }
    let mut store = MapResolver::new();
    let names = restore(&lowered.doc, &mut store, &Names::new());
    let base = name.trim_end_matches('_');
    let hashes: Vec<&String> = names.iter().filter(|(_, n)| n.as_str() == base).map(|(h, _)| h).collect();
    let given: BTreeMap<&str, &str> = lowered
        .doc
        .nodes
        .iter()
        .flat_map(|n| n.data.params.iter().map(move |p| (n, p)))
        .filter_map(|(n, p)| match p {
            crate::wire::WParam::Frozen { value, .. } if hashes.contains(&value) => Some((n.id.as_str(), n.data.name.as_str())),
            _ => None,
        })
        .collect();
    given.iter().find_map(|(id, leaf)| lowered.spans.get(*id).map(|s| (leaf.to_string(), *s)))
}

/// The span of `function name(…)`'s name in `src`.
fn function_span(src: &str, name: &str) -> Option<Span> {
    ts::parse(src).ok()?.items.iter().find_map(|i| match i {
        Item::Holon(h) if h.name.text == name => Some(h.name.span),
        _ => None,
    })
}

/// The span of the `flow(…)` call, where a mistake about the whole flow is said; else the start.
fn flow_span(src: &str) -> Span {
    let at = ts::parse(src).ok().and_then(|f| f.headers.iter().find(|h| matches!(h.kind, HeaderKind::Flow(_))).map(|h| h.span));
    at.unwrap_or(START)
}

#[cfg(test)]
mod tests {
    use crate::lang::compile;
    use crate::samples;

    /// What the round trip costs: the largest sample read alone, and read with its steps written
    /// back and read again, five times each. A measure, not a check: run it with `--ignored
    /// --nocapture`, in release.
    #[test]
    #[ignore]
    fn what_the_round_trip_costs() {
        let largest = samples::samples().iter().max_by_key(|s| s.text.len()).expect("a sample");
        let time = |f: &dyn Fn()| {
            let mut ms: Vec<f64> = (0..5)
                .map(|_| {
                    let at = std::time::Instant::now();
                    f();
                    at.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            ms.sort_by(f64::total_cmp);
            ms
        };
        let read = time(&|| drop(crate::lang::ts::compile(largest.text).unwrap()));
        let both = time(&|| drop(compile(largest.text).unwrap()));
        println!("{} ({} bytes): read alone {read:.1?} ms, written back too {both:.1?} ms", largest.name, largest.text.len());
    }

    /// Every sample is taken: its steps write back as a text of the same steps.
    #[test]
    fn every_sample_is_written_back() {
        for s in samples::samples() {
            if let Err(e) = compile(s.text) {
                panic!("{}: {e}", s.name);
            }
        }
    }

    /// Small flows at the edges of what a function, a list's function, a branch and a return can
    /// hold: taken when the way back writes them as the same steps, refused (with the mistake, at
    /// its line) when it can't.
    #[test]
    fn a_text_is_taken_only_when_written_back() {
        let refused = |line: u32, said: &str| Some((line, said.to_string()));
        // Each row: what it is, its text, and the line and mistake it is refused with (none: taken).
        type Row<'a> = (&'a str, &'a str, Option<(u32, String)>);
        let rows: &[Row] = &[
            (
                "a function that starts an action inside its if, called under an if",
                include_str!("../../tests/agree/function-under-if.holon.ts"),
                refused(13, "the function \"ask\" starts another flow's action: a function only computes, so start it in the route or action that calls \"ask\""),
            ),
            ("a function that writes, called in a try that fails", include_str!("../../tests/agree/failed-try-keeps-function-write.holon.ts"), None),
            (
                "a function that writes, called by the function given to map",
                include_str!("../../tests/agree/list-function-write-dropped.holon.ts"),
                refused(17, "the function given to map has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: move it out of the function, into the route or action itself"),
            ),
            (
                "the function given to map starts an action, below a return",
                include_str!("../../tests/agree/list-start-after-return.holon.ts"),
                refused(15, "the function given to map has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: move it out of the function, into the route or action itself"),
            ),
            (
                "a function whose start is one side of a choice",
                include_str!("../../tests/agree/choice-inside-function.holon.ts"),
                refused(13, "the function \"maybe\" has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: move it into the route or action that calls \"maybe\""),
            ),
            ("a write in a finally, below a return", include_str!("../../tests/agree/finally-after-return.holon.ts"), None),
            ("an action answering early, above its wait", include_str!("../../tests/agree/early-answer-above-wait.holon.ts"), None),
            ("a choice whose test reads a collection, below a return", include_str!("../../tests/agree/choice-test-read-after-return.holon.ts"), None),
            ("a start in a choice, its result read in a branch not taken", include_str!("../../tests/agree/call-read-only-in-untaken-branch.holon.ts"), None),
            ("a send reading a collection, below a return", include_str!("../../tests/agree/read-after-return.holon.ts"), None),
            ("a write on the line after one that fails", include_str!("../../tests/agree/line-after-failure-no-try.holon.ts"), None),
            ("a key written again in an if on nothing", include_str!("../../tests/agree/if-on-nothing-undoes-write.holon.ts"), None),
        ];
        let mut wrong = Vec::new();
        for (what, src, want) in rows {
            let got = compile(src).err().map(|e| (e.span.start.line, e.message));
            if got != *want {
                wrong.push(format!("{what}:\n  want {want:?}\n  got  {got:?}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
