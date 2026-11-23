//! Where each doing site of a text is reached, read off a run (step C1 of the one rule for
//! reached, `lang::reached`): every sample's handlers run in-process as the golden record runs
//! them (golden.rs), each site's path read on the values the run computed. Today's graph holds a
//! step back in its own ways; a test of the path the run never computed is "not known", and
//! counted.

use std::collections::{BTreeMap, HashMap};

use holon_core::{eval_observed, parse_hash, Observer, Resolver, Slot, Value};

use crate::composites::Names;
use crate::golden::{bodies, request};
use crate::lang::ast::{Expr, ExprKind, Span};
use crate::lang::reached::{holds, Atom, Seen, Site};
use crate::lang::{ts, Lowered};
use crate::samples::samples;
use holon_core::MapResolver;

/// What one run computed: each of the handler's own steps' outputs, by its place among them.
#[derive(Default)]
struct Run {
    produced: HashMap<u32, Vec<Slot>>,
}

impl Observer for Run {
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        if let [0, k] = path {
            self.produced.insert(*k, outputs.to_vec());
        }
    }
}

/// A text compiled with its sites, each site's test parts and what each was lowered to.
pub(crate) struct Watched {
    pub lowered: Lowered,
    pub parts: Vec<Expr>,
    pub found: Vec<Vec<(String, usize)>>,
}

impl Watched {
    pub fn of(src: &str) -> Watched {
        let (lowered, parts, found) = ts::compile_watched(src).unwrap_or_else(|e| panic!("{e}"));
        Watched { lowered, parts, found }
    }

    /// The nodes a part was lowered to; a binding's name read nowhere, its own node.
    fn nodes(&self, part: &Expr) -> Vec<(String, usize)> {
        let i = self.parts.iter().position(|p| p == part).expect("a part of a test");
        if !self.found[i].is_empty() {
            return self.found[i].clone();
        }
        match &part.kind {
            ExprKind::Name(n) => self.lowered.names.iter().filter(|(_, b)| *b == n).map(|(id, _)| (id.clone(), 0)).collect(),
            _ => vec![],
        }
    }
}

/// One handler of a text: its label, its unit, its steps by node id, and where it is written.
struct Handler {
    label: String,
    hash: String,
    inputs: Vec<Vec<Option<Value>>>,
    child: HashMap<String, u32>,
}

/// Whether `inner` is written within `outer`.
fn within(inner: Span, outer: Span) -> bool {
    let at = |p: crate::lang::ast::Pos| (p.line, p.col);
    at(outer.start) <= at(inner.start) && at(inner.end) <= at(outer.end)
}

/// What a site's path gives on one run: reached, not, or not known (the first test it needs the
/// run never computed), and the tests looked at.
#[derive(Debug, PartialEq)]
enum Reached {
    Yes,
    No,
    /// At the atom of this index.
    NotKnown(usize),
    /// Entered from elsewhere: a function's body, or a function written in place.
    Elsewhere,
}

fn path_on(site: &Site, w: &Watched, h: &Handler, run: &Run, unknown_parts: &mut Vec<Expr>) -> Reached {
    for (i, atom) in site.path.iter().enumerate() {
        let ok = match atom {
            Atom::Entry => Some(true),
            Atom::Entered(_) | Atom::Item { .. } => return Reached::Elsewhere,
            // Which steps throw in JavaScript is the comparison's table, the next step.
            Atom::Completed(_) => Some(true),
            Atom::Holds { test, .. } => holds(test, &mut |part| {
                let seen = w
                    .nodes(part)
                    .iter()
                    .filter_map(|(id, port)| h.child.get(id).and_then(|k| run.produced.get(k)).map(|outs| outs.get(*port).cloned().unwrap_or(Slot::Absent)))
                    .map(|slot| match slot {
                        Slot::Value(v) => Seen::Value(v),
                        _ => Seen::Absent,
                    })
                    .next();
                seen.unwrap_or_else(|| {
                    if !unknown_parts.contains(part) {
                        unknown_parts.push(part.clone());
                    }
                    Seen::Unknown
                })
            }),
        };
        match ok {
            Some(true) => {}
            Some(false) => return Reached::No,
            None => return Reached::NotKnown(i),
        }
    }
    Reached::Yes
}

/// The handlers a text's server has, each with golden's inputs.
fn handlers(w: &Watched, store: &mut MapResolver) -> Vec<Handler> {
    let doc = &w.lowered.doc;
    crate::text::restore(doc, store, &Names::new());
    let port = doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
    let server = crate::flow::derive_server(&doc.nodes, &doc.edges, store, &port, doc.unit_settings());
    let trace = |label: &str| server.trace.get(label).map(|ids| ids.iter().enumerate().map(|(k, id)| (id.clone(), k as u32)).collect()).unwrap_or_default();
    let mut out = Vec::new();
    for e in &server.endpoints {
        let label = format!("{} {}", e.method, e.path);
        let inputs = bodies().iter().map(|b| vec![Some(request(&e.method, &e.path, b))]).collect();
        out.push(Handler { child: trace(&label), label, hash: e.hash.clone(), inputs });
    }
    for a in &server.actions {
        let inputs = bodies().iter().map(|b| vec![Some(holon_core::parse_json(if b.is_empty() { "{}" } else { b }).unwrap_or(Value::Record(BTreeMap::new())))]).collect();
        out.push(Handler { child: trace(&a.label), label: a.label.clone(), hash: a.hash.clone(), inputs });
    }
    for c in &server.clocks {
        out.push(Handler { child: trace(&c.label), label: c.label.clone(), hash: c.hash.clone(), inputs: vec![vec![]] });
    }
    out
}

/// The census at step C1's first stop: how many sites each sample has, and how many of the tests
/// on their paths today's runs never compute. Printed; it asserts only that watching the
/// lowering changes nothing it gives.
#[test]
fn every_samples_sites_are_read_off_its_runs() {
    let mut report = String::new();
    let (mut sites_all, mut by_kind, mut elsewhere_all) = (0, BTreeMap::<&str, usize>::new(), 0);
    let (mut parts_all, mut parts_unlowered, mut parts_never) = (0, 0, 0);
    let (mut looked, mut yes, mut no, mut not_known) = (0, 0, 0, 0);
    let mut unowned = 0;
    for s in samples() {
        let w = Watched::of(s.text);
        let plain = ts::compile(s.text).unwrap();
        assert!(plain.doc == w.lowered.doc, "{}: watching the lowering changed what it gives", s.name);
        let mut store = MapResolver::new();
        let hs = handlers(&w, &mut store);
        let mut unknown_parts: Vec<Expr> = Vec::new();
        sites_all += w.lowered.sites.len();
        for site in &w.lowered.sites {
            *by_kind.entry(site.does).or_default() += 1;
        }
        parts_all += w.parts.len();
        parts_unlowered += w.parts.iter().filter(|p| w.nodes(p).is_empty()).count();
        // Each site, under the handler whose steps are written within its body.
        let mut owned: Vec<Vec<&Site>> = hs.iter().map(|_| Vec::new()).collect();
        for site in &w.lowered.sites {
            if site.path.iter().any(|a| matches!(a, Atom::Entered(_) | Atom::Item { .. })) {
                elsewhere_all += 1;
                continue;
            }
            let mut any = false;
            for (h, sites) in hs.iter().zip(owned.iter_mut()) {
                if h.child.keys().any(|id| w.lowered.spans.get(id).is_some_and(|sp| within(*sp, site.within))) {
                    sites.push(site);
                    any = true;
                }
            }
            if !any {
                unowned += 1;
                let _ = std::fmt::Write::write_fmt(&mut report, format_args!("  {}: no handler holds the site at {:?} ({})\n", s.name, site.at.start, site.does));
            }
        }
        for (h, sites) in hs.iter().zip(&owned) {
            let hash = parse_hash(&h.hash).unwrap();
            let unit = store.resolve(&hash).unwrap_or_else(|| panic!("{} {}: unit", s.name, h.label));
            for input in &h.inputs {
                let mut run = Run::default();
                eval_observed(unit, input, &store, &mut run);
                for site in sites {
                    looked += 1;
                    match path_on(site, &w, h, &run, &mut unknown_parts) {
                        Reached::Yes => yes += 1,
                        Reached::No => no += 1,
                        Reached::NotKnown(_) => not_known += 1,
                        Reached::Elsewhere => unreachable!("left out above"),
                    }
                }
            }
        }
        parts_never += unknown_parts.len();
        for p in &unknown_parts {
            let line = s.text.lines().nth(p.span.start.line as usize - 1).unwrap_or("").trim();
            let _ = std::fmt::Write::write_fmt(&mut report, format_args!("  {}: never computed where needed, line {}: {line}\n", s.name, p.span.start.line));
        }
    }
    println!(
        "sites: {sites_all} ({by_kind:?}); in a function's body or a function written in place: {elsewhere_all}; in no handler: {unowned}\n\
         test parts: {parts_all}, lowered to no step of today's graph: {parts_unlowered}, never computed on some run where a path needed it: {parts_never}\n\
         site runs: {looked}: reached {yes}, not reached {no}, not known {not_known}\n{report}"
    );
}
