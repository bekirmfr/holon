//! A canvas drawn by hand, kept beside its flow's text (docs/L4s/text-deploy-plan.md).
//!
//! A flow travels and is kept as its text; its canvas is the text compiled, laid out by the
//! compiler. A canvas a person arranged by hand keeps its arrangement as a [`Layout`]: the
//! position of each of its nodes, handler by handler, in the order the handler's `trace` lists
//! them (the canonical order of its unit's children, so it doesn't depend on the names the compiler
//! gives nodes), and the unit it was drawn for. A layout is put back only on that unit's canvas:
//! code compiled otherwise (the text edited, or another release's compiler) is laid out again, and
//! whoever shows it says so. Positions are presentation: they never reach a unit or its hash.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The most positions a layout holds: a canvas of a flow far past any written so far.
pub const MAX_PLACES: usize = 200_000;
/// The most handlers a layout names, and the longest label.
const MAX_HANDLERS: usize = 10_000;
const MAX_LABEL: usize = 1_000;
/// How far from the origin a position may be.
const FAR: f64 = 1e9;

/// Where a hand-drawn canvas's nodes are: `at[label][i]` is the position of the `i`th node of the
/// handler `label` in its trace, on the canvas of `unit` (hex).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Layout {
    pub unit: String,
    pub at: BTreeMap<String, Vec<[f64; 2]>>,
}

/// What [`apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    /// Every node is where it was drawn.
    AsDrawn,
    /// The layout was drawn for other code: the canvas keeps the compiler's layout.
    OtherCode,
}

/// Whether `layout` is one a server keeps: a unit's hash, at most [`MAX_PLACES`] positions in at
/// most [`MAX_HANDLERS`] handlers, each a pair of finite numbers near the origin.
pub fn check(layout: &Layout) -> Result<(), String> {
    if layout.unit.len() != 64 || !layout.unit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("a layout names the unit it was drawn for, as a 64-character hash".into());
    }
    if layout.at.len() > MAX_HANDLERS {
        return Err(format!("a layout places the nodes of at most {MAX_HANDLERS} handlers"));
    }
    if layout.at.keys().any(|label| label.len() > MAX_LABEL) {
        return Err(format!("a layout's handler labels are at most {MAX_LABEL} bytes"));
    }
    let places: usize = layout.at.values().map(Vec::len).sum();
    if places > MAX_PLACES {
        return Err(format!("a layout places at most {MAX_PLACES} nodes, and this one places {places}"));
    }
    if layout.at.values().flatten().flatten().any(|n| !n.is_finite() || n.abs() > FAR) {
        return Err("a layout's positions are finite numbers".into());
    }
    Ok(())
}

/// The nodes of `document` (a flow document: `nodes` with ids and positions) by id: their
/// positions.
fn positions(document: &Value) -> BTreeMap<&str, [f64; 2]> {
    let nodes = document.get("nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    nodes
        .iter()
        .filter_map(|n| {
            let id = n.get("id")?.as_str()?;
            let p = n.get("position")?;
            Some((id, [p.get("x")?.as_f64()?, p.get("y")?.as_f64()?]))
        })
        .collect()
}

/// Each handler's node ids (a document's `trace`), in its trace's order.
fn traced(document: &Value) -> BTreeMap<&str, Vec<&str>> {
    let Some(trace) = document.get("trace").and_then(Value::as_object) else { return BTreeMap::new() };
    trace.iter().map(|(label, ids)| (label.as_str(), ids.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default())).collect()
}

/// The layout of `canvas` (a flow document with its nodes' positions and its `trace`), whose code
/// is `unit`, beside `laid_out`, the same code as the compiler lays it out (its text compiled):
/// none when every node is where the compiler puts it (nothing was moved by hand). An error when
/// the two aren't the same handlers with as many nodes each: then they aren't the same code.
pub fn of_canvas(canvas: &Value, unit: &str, laid_out: &Value) -> Result<Option<Layout>, String> {
    let (drawn, compiled) = (traced(canvas), traced(laid_out));
    if drawn.len() != compiled.len() || drawn.iter().any(|(label, ids)| compiled.get(label).is_none_or(|c| c.len() != ids.len())) {
        return Err("the canvas isn't the code its text compiles to: its handlers differ".into());
    }
    let (here, there) = (positions(canvas), positions(laid_out));
    let mut at = BTreeMap::new();
    let mut moved = false;
    for (label, ids) in &drawn {
        let mut places = Vec::with_capacity(ids.len());
        for (i, id) in ids.iter().enumerate() {
            let p = *here.get(id).ok_or_else(|| format!("the canvas's trace names {id:?}, which isn't one of its nodes"))?;
            moved |= compiled.get(label).and_then(|c| there.get(c[i])) != Some(&p);
            places.push(p);
        }
        at.insert(label.to_string(), places);
    }
    let layout = Layout { unit: unit.to_string(), at };
    check(&layout)?;
    Ok(moved.then_some(layout))
}

/// Put `layout` on `document` (a flow's text compiled: its nodes and `trace`), whose code is
/// `unit`: each node where it was drawn ([`Placed::AsDrawn`]), when the layout was drawn for that
/// code; else the document as the compiler laid it out ([`Placed::OtherCode`]), untouched, never
/// some nodes moved and others not.
pub fn apply(document: &mut Value, unit: &str, layout: &Layout) -> Placed {
    if layout.unit != unit {
        return Placed::OtherCode;
    }
    let mut to: BTreeMap<String, [f64; 2]> = BTreeMap::new();
    {
        let traced = traced(document);
        if traced.len() != layout.at.len() {
            return Placed::OtherCode;
        }
        for (label, ids) in &traced {
            match layout.at.get(*label) {
                Some(places) if places.len() == ids.len() => to.extend(ids.iter().zip(places).map(|(id, p)| (id.to_string(), *p))),
                _ => return Placed::OtherCode,
            }
        }
    }
    if let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) {
        for n in nodes {
            let Some(p) = n.get("id").and_then(Value::as_str).and_then(|id| to.get(id)).copied() else { continue };
            n["position"] = serde_json::json!({ "x": p[0], "y": p[1] });
        }
    }
    Placed::AsDrawn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(text: &str) -> (String, Value) {
        let plan = crate::agent::plan(text).unwrap();
        (plan.unit, plan.document)
    }

    const TEXT: &str = "import { flow, route } from \"holon\"\nflow(\"lay\", { port: 8101 })\nroute.get(\"/a/:n\", (req) => `a ${req.params.n}`)\nroute.get(\"/b\", (req) => ({ b: req.query.b ?? \"\" }))\n";

    /// A canvas as the compiler lays it out has no layout; one moved by hand has one, which put on
    /// the text compiled again places every node where it was drawn, and on other code places none.
    #[test]
    fn a_hand_drawn_canvas_comes_back_as_drawn_and_only_on_its_own_code() {
        let (unit, laid_out) = compiled(TEXT);
        assert_eq!(of_canvas(&laid_out, &unit, &laid_out).unwrap(), None, "nothing moved, nothing kept");

        let mut drawn = laid_out.clone();
        for (i, n) in drawn["nodes"].as_array_mut().unwrap().iter_mut().enumerate() {
            n["position"] = serde_json::json!({ "x": 1000.0 + i as f64, "y": -7.5 * i as f64 });
        }
        let layout = of_canvas(&drawn, &unit, &laid_out).unwrap().expect("moved by hand");
        assert_eq!(layout.at.values().map(Vec::len).sum::<usize>(), drawn["nodes"].as_array().unwrap().len(), "every node placed");
        check(&layout).unwrap();

        // Compiled again, the same text: every node where it was drawn.
        let (again, mut reopened) = compiled(TEXT);
        assert_eq!(apply(&mut reopened, &again, &layout), Placed::AsDrawn);
        assert_eq!(positions(&reopened), positions(&drawn));

        // Other code: laid out as the compiler does, untouched.
        let (other, mut edited) = compiled(&TEXT.replace("`a ${req.params.n}`", "`A ${req.params.n}`"));
        let before = edited.clone();
        assert_eq!(apply(&mut edited, &other, &layout), Placed::OtherCode);
        assert_eq!(edited, before);
        // A layout naming this unit but placing other handlers (one cut): none moved.
        let mut cut = layout.clone();
        cut.at.pop_first();
        let mut doc = compiled(TEXT).1;
        assert_eq!(apply(&mut doc, &again, &cut), Placed::OtherCode);
        assert_eq!(doc, compiled(TEXT).1);
    }

    /// What a server keeps of a layout is bounded and well formed.
    #[test]
    fn a_layout_is_bounded_and_finite() {
        let ok = Layout { unit: "a".repeat(64), at: BTreeMap::from([("GET /".to_string(), vec![[1.0, 2.0]])]) };
        check(&ok).unwrap();
        for bad in [
            Layout { unit: "xyz".into(), ..ok.clone() },
            Layout { at: BTreeMap::from([("GET /".to_string(), vec![[f64::NAN, 0.0]])]), ..ok.clone() },
            Layout { at: BTreeMap::from([("GET /".to_string(), vec![[2e9, 0.0]])]), ..ok.clone() },
            Layout { at: BTreeMap::from([("GET /".to_string(), vec![[0.0, 0.0]; MAX_PLACES + 1])]), ..ok.clone() },
            Layout { at: BTreeMap::from([("x".repeat(MAX_LABEL + 1), vec![[0.0, 0.0]])]), ..ok.clone() },
        ] {
            assert!(check(&bad).is_err(), "{:?}", bad.unit);
        }
    }
}
