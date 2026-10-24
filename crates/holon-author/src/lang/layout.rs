//! Where a compiled text's nodes go on the canvas (docs/LANGUAGE.md §8). Positions aren't part
//! of a program, so the text has none: each connected group (a route, a flow's own statements)
//! is a band, the bands stacked in the order the text first writes them; inside a band, a node's
//! column is its depth (the longest chain of wires from a node nothing feeds), a node nothing
//! feeds sits just before the first node it feeds, and a column's nodes keep the text's order top
//! to bottom.

use std::collections::HashMap;

use crate::builder::{fit, FlowDocument};
use crate::canvas::Position;
use crate::routes::components;

const LEFT: f64 = 40.0;
const TOP: f64 = 40.0;
const COLUMN: f64 = 260.0;
const ROW: f64 = 160.0;
/// The room between two bands, over a row's.
const GAP: f64 = 80.0;

/// Lay out the flow's own canvas, and fit the view to it.
pub fn lay_out(doc: &mut FlowDocument) {
    let ids: Vec<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
    let order: HashMap<String, usize> = ids.iter().enumerate().map(|(i, id)| (id.to_string(), i)).collect();
    let mut feeds: HashMap<usize, Vec<usize>> = HashMap::new();
    for w in &doc.edges {
        if let (Some(&s), Some(&t)) = (order.get(&w.source), order.get(&w.target))
            && s != t
        {
            feeds.entry(t).or_default().push(s);
        }
    }
    // Depth by the longest chain into a node; the graph is acyclic, and a guard keeps a
    // malformed document from looping.
    let mut depth: Vec<Option<usize>> = vec![None; ids.len()];
    fn depth_of(i: usize, feeds: &HashMap<usize, Vec<usize>>, depth: &mut [Option<usize>], seen: usize) -> usize {
        if let Some(d) = depth[i] {
            return d;
        }
        if seen > depth.len() {
            return 0;
        }
        let d = feeds.get(&i).map(|from| from.iter().map(|&f| depth_of(f, feeds, depth, seen + 1) + 1).max().unwrap_or(0)).unwrap_or(0);
        depth[i] = Some(d);
        d
    }
    let mut col: Vec<usize> = (0..ids.len()).map(|i| depth_of(i, &feeds, &mut depth, 0)).collect();
    // A node nothing feeds (a constant, a secret, the clock) sits just before the first node it
    // feeds, not in a first column of its own.
    for i in 0..ids.len() {
        if feeds.contains_key(&i) {
            continue;
        }
        if let Some(first) = feeds.iter().filter(|(_, from)| from.contains(&i)).map(|(&t, _)| col[t]).min() {
            col[i] = first.saturating_sub(1);
        }
    }
    let mut groups: Vec<Vec<usize>> = components(&ids, &doc.edges).iter().map(|g| { let mut g: Vec<usize> = g.iter().map(|id| order[id]).collect(); g.sort(); g }).collect();
    groups.sort_by_key(|g| g[0]);
    let mut spot: Vec<Position> = vec![Position::default(); ids.len()];
    let mut top = TOP;
    for group in &groups {
        let mut rows: HashMap<usize, usize> = HashMap::new();
        for &i in group {
            let d = col[i];
            let row = rows.entry(d).or_insert(0);
            spot[i] = Position { x: LEFT + COLUMN * d as f64, y: top + ROW * *row as f64 };
            *row += 1;
        }
        let tallest = rows.values().copied().max().unwrap_or(1);
        top += ROW * tallest as f64 + GAP;
    }
    for (node, at) in doc.nodes.iter_mut().zip(spot) {
        node.position = Some(at);
    }
    doc.viewport = Some(fit(&doc.nodes));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::compile;

    fn at(doc: &FlowDocument, leaf: &str) -> Vec<(f64, f64)> {
        doc.nodes.iter().filter(|n| n.data.name == leaf).map(|n| { let p = n.position(); (p.x, p.y) }).collect()
    }

    #[test]
    fn a_route_reads_left_to_right_and_routes_stack() {
        let doc = compile("flow f\nport 8200\n\nroute GET /a/:x (req) {\n  respond \"{req.params.x}!\"\n}\n\nroute GET /b (req) {\n  respond 204\n}\n").unwrap().doc;
        // The first route: request, then the field, as_text, concat, respond, a column each.
        let (rx, ry) = at(&doc, "request")[0];
        let (fx, _) = at(&doc, "field")[0];
        let (cx, _) = at(&doc, "concat")[0];
        assert_eq!((rx, ry), (LEFT, TOP));
        assert!(rx < fx && fx < cx, "columns by depth");
        let responds = at(&doc, "respond");
        assert!(responds[0].0 > cx, "respond last");
        // The second route below the first, from the left again.
        let second = &at(&doc, "request")[..];
        assert_eq!(second.len(), 2);
        assert_eq!(second[1].0, LEFT);
        assert!(second[1].1 > responds[0].1, "the next band is below");
    }
}
