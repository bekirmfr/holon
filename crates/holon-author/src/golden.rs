//! The evaluator's golden runs: every sample's handlers (and a compute-heavy flow), each with a
//! few fixed inputs, evaluated by core with every node's outputs recorded, then compared with
//! `tests/golden/eval.txt`, the record of what the evaluator gave when it was written. A change to
//! the evaluator that changes any answer or any node's value fails here, naming the case and the
//! line (docs/L4s/interpreter-path-L4.md §4). `HOLON_GOLDEN=update cargo test -p holon-author
//! golden` writes the file afresh, for a change meant to change what it records.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use std::cell::Cell;
use std::sync::Arc;

use holon_core::{eval_observed, hash_unit, parse_hash, Hash, Observer, Resolved, Resolver, Shape, Shapes, Slot, Unit, Value};

use crate::composites::Names;
use crate::lang;
use crate::samples::samples;
use holon_core::MapResolver;

/// A compute-heavy flow besides the samples: statistics over a list, and a record per item.
const CRUNCH: &str = r#"import { flow, route } from "holon"

flow("crunch", { port: 8311 })

route.post("/stats", (req) => {
  const xs = req.json.xs
  const count = xs.reduce((acc, x) => acc + 1 + x * 0, 0)
  const total = xs.reduce((acc, x) => acc + x, 0)
  const squares = xs.map((x) => x * x)
  const sumsq = squares.reduce((acc, x) => acc + x, 0)
  const mean = total / count
  const rows = xs.map((x) => ({ v: x, dev: x - mean, sq: (x - mean) * (x - mean) }))
  const spread = rows.reduce((acc, r) => acc + r.sq, 0)
  return {
    n: count,
    sum: total,
    mean,
    variance: sumsq / count - mean * mean,
    spread: spread / count,
    rows,
  }
})
"#;

/// A value as the record shows it: its `Debug` form (deterministic: records are ordered), cut
/// past 160 characters with the hash of the whole.
fn shown(v: &Value) -> String {
    let full = format!("{v:?}");
    if full.chars().count() <= 160 {
        return full;
    }
    let hash = holon_core::hash_value(v).map(|h| holon_core::hex(&h[..8])).unwrap_or_else(|_| "?".into());
    format!("{}… #{hash}", full.chars().take(160).collect::<String>())
}

/// Every node's outputs as it gave them, in the order they were given.
struct Steps(String);

impl Observer for Steps {
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        let outs: Vec<String> = outputs
            .iter()
            .map(|o| match o {
                Slot::Value(v) => shown(v),
                Slot::Absent => "·".into(),
                Slot::Pending => "…".into(),
            })
            .collect();
        let _ = writeln!(self.0, "  {path:?} {}", outs.join(" | "));
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// A request as the data plane gives a route one.
fn request(method: &str, path: &str, body: &str) -> Value {
    let params: BTreeMap<String, Value> = path.split('/').filter_map(|seg| seg.strip_prefix(':')).map(|k| (k.to_string(), text("tea"))).collect();
    Value::Record(BTreeMap::from([
        ("method".to_string(), text(method)),
        ("path".to_string(), text(path)),
        ("params".to_string(), Value::Record(params)),
        ("query".to_string(), Value::Record(BTreeMap::from([("after".to_string(), text("a"))]))),
        ("headers".to_string(), Value::Record(BTreeMap::from([("content-type".to_string(), text("application/json"))]))),
        ("body".to_string(), text(body)),
    ]))
}

/// The bodies each handler is given: none, a record with the fields samples read, and a list.
fn bodies() -> Vec<String> {
    let xs: Vec<String> = (0..60).map(|i| format!("{}.25", (i * 37) % 100)).collect();
    vec![
        String::new(),
        r#"{"id":"x","name":"tea","price":4,"amount":5,"qty":2,"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}],"event":"gig","count":2,"text":"Hello World","title":"Hello World","author":"Ada","op":"mul","a":6,"b":7,"by":"ops","to":5}"#.to_string(),
        format!(r#"{{"xs":[{}],"items":[]}}"#, xs.join(",")),
    ]
}

/// A handler to evaluate: its label, its unit's hash, and each input it is given.
type Case = (String, String, Vec<Vec<Option<Value>>>);

/// A store with each composite's shape and children kept, as the server keeps them
/// (holon/src/units.rs), counting the runs that found them.
struct Kept<'a> {
    units: BTreeMap<Hash, Arc<Unit>>,
    shapes: Shapes,
    found: &'a Cell<usize>,
}

impl<'a> Kept<'a> {
    fn over(store: &MapResolver, found: &'a Cell<usize>) -> Kept<'a> {
        let units: BTreeMap<Hash, Arc<Unit>> = store.units().map(|u| (hash_unit(u).expect("hashed"), Arc::new(u.clone()))).collect();
        let shapes = Shapes::kept(units.values(), |h| units.get(h));
        Kept { units, shapes, found }
    }
}

impl Resolver for Kept<'_> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.units.get(hash).map(|u| u.as_ref())
    }
    fn shape(&self, unit: &Unit) -> Option<&Shape> {
        let shape = self.shapes.get(unit);
        self.found.set(self.found.get() + shape.is_some() as usize);
        shape
    }
    fn resolved(&self, unit: &Unit) -> Option<&Resolved> {
        let resolved = self.shapes.resolved(unit);
        self.found.set(self.found.get() + resolved.is_some() as usize);
        resolved
    }
}

/// The whole record, for every flow, handler and input; with each composite's shape kept and
/// found, when `found` counts how often, or worked out for each run.
fn record(found: Option<&Cell<usize>>) -> String {
    let mut flows: Vec<(String, String)> = samples().iter().map(|s| (s.name.to_string(), s.text.to_string())).collect();
    flows.push(("crunch".into(), CRUNCH.into()));
    let mut out = String::new();
    for (name, source) in flows {
        let lowered = lang::ts::compile(&source).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut store = MapResolver::new();
        crate::text::restore(&lowered.doc, &mut store, &Names::new());
        let port = lowered.doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
        let server = crate::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port);
        let mut cases: Vec<Case> = Vec::new();
        for e in &server.endpoints {
            let inputs = bodies().iter().map(|b| vec![Some(request(&e.method, &e.path, b))]).collect();
            cases.push((format!("{} {}", e.method, e.path), e.hash.clone(), inputs));
        }
        for a in &server.actions {
            let inputs = bodies().iter().map(|b| vec![Some(holon_core::parse_json(if b.is_empty() { "{}" } else { b }).unwrap_or(Value::Record(BTreeMap::new())))]).collect();
            cases.push((a.label.clone(), a.hash.clone(), inputs));
        }
        for c in &server.clocks {
            cases.push((c.label.clone(), c.hash.clone(), vec![vec![]]));
        }
        let kept = found.map(|found| Kept::over(&store, found));
        for (label, hash, inputs) in cases {
            let hash = parse_hash(&hash).unwrap();
            let unit = store.resolve(&hash).unwrap_or_else(|| panic!("{name} {label}: unit"));
            for (i, input) in inputs.iter().enumerate() {
                let mut steps = Steps(String::new());
                let outputs = match &kept {
                    // The handler as the kept store holds it, so its own children are found too.
                    Some(kept) => eval_observed(kept.resolve(&hash).expect("kept"), input, kept, &mut steps),
                    None => eval_observed(unit, input, &store, &mut steps),
                };
                let shown_out: Vec<String> = outputs.iter().map(|o| o.as_ref().map_or("·".into(), shown)).collect();
                let _ = writeln!(out, "== {name} · {label} · input {i}");
                out.push_str(&steps.0);
                let _ = writeln!(out, "  => {}", shown_out.join(" | "));
            }
        }
    }
    out
}

#[test]
fn the_evaluator_gives_what_its_golden_record_holds() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/eval.txt");
    let now = record(None);
    if std::env::var("HOLON_GOLDEN").as_deref() == Ok("update") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &now).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e} (HOLON_GOLDEN=update writes it)", path.display()));
    if kept == now {
        return;
    }
    // The first line that differs, under the case it belongs to.
    let (mut case, mut line) = (String::from("?"), 0);
    for (i, (k, n)) in kept.lines().zip(now.lines()).enumerate() {
        if k.starts_with("== ") {
            case = k.to_string();
        }
        if k != n {
            line = i + 1;
            panic!("the evaluator gives something else at line {line} of {}, in {case}:\n  kept: {k}\n  now:  {n}", path.display());
        }
    }
    panic!("the record has {} lines, the evaluator now {} (last case: {case}, line {line})", kept.lines().count(), now.lines().count());
}

/// Keeping each composite's shape and children across runs (docs/L4s/interpreter-path-L4.md, step
/// 1b; js-library-races/leaves.md §8.1) gives the very record working them out for each run gives.
#[test]
fn keeping_each_composites_shape_changes_nothing_the_evaluator_gives() {
    let found = Cell::new(0);
    let kept = record(Some(&found));
    assert!(found.get() > 1000, "the kept shapes and children were found {} times", found.get());
    assert!(kept == record(None), "a run with the shapes kept gives something else");
}

/// Every function the compute-heavy flow's `map`s and `fold`s apply runs down a plan (D5): the
/// language shares an input through a pass-through the plan takes, and a change to how it lowers
/// one would leave them walked, correct but slower, without this failing.
#[test]
fn the_compute_heavy_flows_functions_all_run_down_a_plan() {
    let lowered = lang::ts::compile(CRUNCH).unwrap();
    let mut store = MapResolver::new();
    crate::text::restore(&lowered.doc, &mut store, &Names::new());
    let port = lowered.doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
    crate::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port);
    let mut functions = Vec::new();
    for unit in store.units() {
        if let holon_core::UnitBody::Primitive { name } = &unit.body
            && holon_core::PASSES_EXTRA.contains(&name.as_str())
            && let Some(f) = unit.params.iter().find(|p| p.name() == "fn").and_then(|p| p.value()).and_then(parse_hash)
        {
            functions.push(f);
        }
    }
    // Six in the flow; two fold with the same function, one unit.
    assert!(functions.len() >= 5, "the flow's maps and folds: {}", functions.len());
    for f in functions {
        let unit = store.resolve(&f).expect("the function resolves");
        assert!(holon_core::would_plan(unit, &store), "the function {} is walked, not planned", holon_core::hex(f));
    }
}
