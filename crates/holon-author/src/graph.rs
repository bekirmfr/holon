//! A flow as a graph a terminal can draw (`holon editor`'s Graph): not its nodes, which are many
//! and mostly plumbing (a `field`, a `const`, a `with_field`), but its statements, each a row,
//! with the wires between them; the nodes a statement is made of are its detail. Each trigger (a
//! route, a clock, an action, a file handler) is a band of its own, headed by its trigger, as the
//! canvas stacks them. [`lanes`] draws a band's wires as `git log --graph` draws history: a lane
//! from each row down to the last row it feeds.

use std::collections::{BTreeMap, HashMap};

use crate::lang::ast::{Item, Pos, Span, Stmt, StmtKind};
use crate::lang::{lower, ts};
use crate::wire::WParam;

/// A placed node, as a statement's detail.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Its name in the text, else its id.
    pub name: String,
    /// What it is: a leaf's name, a template's (`request`, `respond`), a holon's.
    pub leaf: String,
    /// Its params as set: name and value.
    pub params: Vec<(String, String)>,
    /// What feeds each input: the port, then the node and its output (`account.out`).
    pub inputs: Vec<(String, String)>,
}

/// A row: a statement (or, with every node shown, a node), or a band's trigger.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Its first and last line in the text, 1-based.
    pub line: u32,
    pub last: u32,
    /// What it is called: its binding, `state.n`, `respond`, what it calls.
    pub name: String,
    /// What its outermost nodes are (`db_one`, `respond`), or the trigger's.
    pub does: String,
    /// Its first line, trimmed.
    pub text: String,
    /// Where the text says it, to mark when the row is picked: a statement's name (its
    /// binding, the state or collection it writes, what it calls), a node's own code, the
    /// trigger as its line writes it.
    pub mark: Span,
    /// The band's trigger: it heads the band.
    pub head: bool,
    /// It acts on the world (writes, answers, calls out, waits), not only computes.
    pub effect: bool,
    /// The rows of this band it takes from, by index (only earlier rows, as the text writes).
    pub from: Vec<usize>,
    /// What it takes from outside its band, by name.
    pub outside: Vec<String>,
    pub parts: Vec<Part>,
}

/// One trigger and the statements it runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// The trigger as the text writes it: `route POST /notes/:id (req)`, `every 5s`.
    pub title: String,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Graph {
    pub bands: Vec<Band>,
    /// How many nodes it has in all.
    pub nodes: usize,
}

fn inside(p: Pos, s: Span) -> bool {
    (p.line, p.col) >= (s.start.line, s.start.col) && (p.line, p.col) < (s.end.line, s.end.col)
}

/// A handler's statements, with the trigger's label and the handler's span.
struct Block<'a> {
    title: String,
    span: Span,
    body: Vec<&'a Stmt>,
}

/// The text's graph: by statement, or with `every` node a row of its own.
pub fn graph(source: &str, every: bool) -> Result<Graph, String> {
    let file = ts::parse(source).map_err(|e| e.to_string())?;
    let lowered = lower(&file).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = source.lines().collect();
    let line_text = |n: u32| lines.get(n as usize - 1).map(|l| l.trim()).unwrap_or_default().to_string();
    // `len` characters from `from` on its line.
    let run = |from: Pos, len: usize| Span { start: from, end: Pos { line: from.line, col: from.col + len as u32 } };
    // A line's text, its indent left out.
    let whole = |n: u32| {
        let l = lines.get(n as usize - 1).copied().unwrap_or_default();
        let indent = l.chars().count() - l.trim_start().chars().count();
        run(Pos { line: n, col: indent as u32 + 1 }, l.trim().chars().count())
    };
    // Each handler is titled as the server labels its trigger.
    let mut all: Vec<Block> = vec![];
    for item in &file.items {
        let (title, span, body) = match item {
            Item::Route(r) => (format!("{} {}", r.method.text, r.path.text), r.span, &r.body),
            Item::Every(e) => (holon_core::clock_label(e.seconds as u64), e.span, &e.body),
            Item::OnFile(o) => (holon_core::FileTrigger { folder: o.on.folder.clone(), deleted: o.on.deleted, matching: o.on.matching.clone().unwrap_or_default() }.label(), o.span, &o.body),
            Item::Action(a) => (format!("action {}", a.name.text), a.span, &a.body),
            Item::Stmt(_) | Item::Holon(_) | Item::Js(_) => continue,
        };
        all.push(Block { title, span, body: body.iter().collect() });
    }

    // Each node to its block and statement (none: the block's trigger).
    let doc = &lowered.doc;
    let mut place: HashMap<&str, (usize, Option<usize>)> = HashMap::new();
    for n in &doc.nodes {
        let Some(span) = lowered.spans.get(&n.id) else { continue };
        let p = span.start;
        let found = all.iter().enumerate().find_map(|(b, block)| {
            // A statement is its stretch of the text, up to the next one: a `const`'s span is
            // its name alone.
            let stretch = |s: usize| Span { start: block.body[s].span.start, end: block.body.get(s + 1).map_or(block.span.end, |n| n.span.start) };
            if let Some(s) = (0..block.body.len()).find(|&s| inside(p, stretch(s))) {
                return Some((b, Some(s)));
            }
            inside(p, block.span).then_some((b, None))
        });
        if let Some(at) = found {
            place.insert(n.id.as_str(), at);
        }
    }
    let name_of = |id: &str| lowered.names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let part_of = |id: &str| -> Option<Part> {
        let n = doc.nodes.iter().find(|n| n.id == id)?;
        let params = n.data.params.iter().filter_map(|p| match p {
            WParam::Frozen { name, value } if !value.is_empty() => Some((name.clone(), value.clone())),
            _ => None,
        });
        let inputs = doc.edges.iter().filter(|w| w.target == id).map(|w| (w.input().to_string(), format!("{}.{}", name_of(&w.source), w.output())));
        Some(Part { name: name_of(id), leaf: n.data.name.clone(), params: params.collect(), inputs: inputs.collect() })
    };
    let effectful = |leaf: &str| leaf.starts_with("http") || leaf.starts_with("db_") || leaf == "await" || leaf == "approval" || leaf == "respond";

    let mut bands = vec![];
    for (b, block) in all.iter().enumerate() {
        // The rows' keys: the trigger, then each statement (or each node of it).
        let mut rows: Vec<Row> = vec![];
        let mut row_of: HashMap<&str, usize> = HashMap::new();
        let trigger: Vec<&str> = doc.nodes.iter().map(|n| n.id.as_str()).filter(|id| place.get(id) == Some(&(b, None))).collect();
        let head_line = block.span.start.line;
        rows.push(Row {
            line: head_line,
            last: head_line,
            name: block.title.clone(),
            does: trigger.iter().filter_map(|id| doc.nodes.iter().find(|n| n.id == *id)).map(|n| n.data.name.clone()).collect::<Vec<_>>().join(", "),
            text: block.title.clone(),
            mark: whole(head_line),
            head: true,
            effect: false,
            from: vec![],
            outside: vec![],
            parts: trigger.iter().filter_map(|id| part_of(id)).collect(),
        });
        for id in &trigger {
            row_of.insert(id, 0);
        }
        for (s, stmt) in block.body.iter().enumerate() {
            let ids: Vec<&str> = doc.nodes.iter().map(|n| n.id.as_str()).filter(|id| place.get(id) == Some(&(b, Some(s)))).collect();
            if ids.is_empty() {
                continue;
            }
            let text = line_text(stmt.span.start.line);
            let end = ids.iter().filter_map(|id| lowered.spans.get(*id)).map(|s| s.end).chain([stmt.span.end]).max_by_key(|p| (p.line, p.col)).unwrap_or(stmt.span.end);
            let last = if end.col == 1 && end.line > stmt.span.start.line { end.line - 1 } else { end.line };
            let name = match &stmt.kind {
                StmtKind::Bind { name, .. } => name.text.clone(),
                StmtKind::StateWrite { key, .. } => format!("state.{}", key.text),
                StmtKind::CollectionWrite { collection, .. } => format!("{}[…]", collection.text),
                StmtKind::Delete { collection, .. } => format!("delete {}", collection.text),
                StmtKind::Respond(_) => "return".into(),
                StmtKind::Expr(_) => text.trim_start_matches("await ").split(['(', ' ']).next().unwrap_or_default().to_string(),
            };
            let start = stmt.span.start;
            // Where the statement's own words begin on its line: an arrow's answer has no
            // `return`, and an effect may be awaited.
            let said_at = |word: &str| lines.get(start.line as usize - 1).and_then(|l| l.find(word).map(|i| l[..i].chars().count() as u32));
            let mark = match &stmt.kind {
                StmtKind::Bind { name, .. } => name.span,
                StmtKind::StateWrite { key, .. } => Span { start, end: key.span.end },
                StmtKind::CollectionWrite { collection, .. } => collection.span,
                StmtKind::Delete { collection, .. } => Span { start, end: collection.span.end },
                StmtKind::Respond(_) | StmtKind::Expr(_) => match said_at(&name) {
                    Some(i) => run(Pos { line: start.line, col: i + 1 }, name.chars().count()),
                    None => whole(start.line),
                },
            };
            let bind = matches!(stmt.kind, StmtKind::Bind { .. });
            let leaves = |of: &[&str]| -> Vec<String> { of.iter().filter_map(|id| doc.nodes.iter().find(|n| n.id == *id)).map(|n| n.data.name.clone()).collect() };
            let effect = !bind || leaves(&ids).iter().any(|l| effectful(l));
            if every {
                for id in &ids {
                    let part = part_of(id).unwrap_or_else(|| Part { name: id.to_string(), leaf: String::new(), params: vec![], inputs: vec![] });
                    let said: Vec<String> = part.params.iter().map(|(k, v)| format!("{k}={}", serde_json::to_string(v).unwrap_or_default())).collect();
                    row_of.insert(id, rows.len());
                    rows.push(Row {
                        line: stmt.span.start.line,
                        last,
                        name: part.name.clone(),
                        does: part.leaf.clone(),
                        text: if said.is_empty() { text.clone() } else { said.join(" ") },
                        mark: lowered.spans.get(*id).copied().unwrap_or(mark),
                        head: false,
                        effect: effectful(&part.leaf),
                        from: vec![],
                        outside: vec![],
                        parts: vec![part],
                    });
                }
                continue;
            }
            // Its outermost nodes: those feeding nothing else of the statement.
            let roots: Vec<&str> = ids.iter().copied().filter(|id| !doc.edges.iter().any(|w| w.source == *id && ids.contains(&w.target.as_str()))).collect();
            for id in &ids {
                row_of.insert(id, rows.len());
            }
            rows.push(Row {
                line: stmt.span.start.line,
                last,
                name,
                does: leaves(&roots).join(", "),
                text,
                mark,
                head: false,
                effect,
                from: vec![],
                outside: vec![],
                parts: ids.iter().filter_map(|id| part_of(id)).collect(),
            });
        }
        // The wires between rows, each once, in the order the rows come.
        let mut from: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut outside: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for w in &doc.edges {
            let Some(&t) = row_of.get(w.target.as_str()) else { continue };
            match row_of.get(w.source.as_str()) {
                Some(&s) if s != t => from.entry(t).or_default().push(s),
                Some(_) => {}
                None => outside.entry(t).or_default().push(name_of(&w.source)),
            }
        }
        for (t, mut f) in from {
            f.sort();
            f.dedup();
            rows[t].from = f;
        }
        for (t, mut o) in outside {
            o.sort();
            o.dedup();
            rows[t].outside = o;
        }
        bands.push(Band { title: block.title.clone(), rows });
    }
    Ok(Graph { bands, nodes: doc.nodes.len() })
}

/// A box-drawing joint with lines going up, down, left and right of it.
fn joint(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (true, true, true, true) => '┼',
        (true, true, false, true) => '├',
        (true, true, true, false) => '┤',
        (true, false, true, true) => '┴',
        (false, true, true, true) => '┬',
        (true, false, false, true) => '╰',
        (true, false, true, false) => '╯',
        (false, true, false, true) => '╭',
        (false, true, true, false) => '╮',
        (true, _, false, false) | (false, true, false, false) => '│',
        (false, false, true, _) | (false, false, _, true) => '─',
        (false, false, false, false) => ' ',
    }
}

/// Each row's wires, drawn as lanes: a row that feeds later rows has a lane down to the last of
/// them; a row is a dot on the lowest lane free there, joined across to the lanes of the rows it
/// takes from. All the same width, in characters.
pub fn lanes(rows: &[Row]) -> Vec<String> {
    let n = rows.len();
    let mut last: Vec<Option<usize>> = vec![None; n];
    let mut into: Vec<Vec<usize>> = vec![vec![]; n];
    for (t, r) in rows.iter().enumerate() {
        for &s in r.from.iter().filter(|&&s| s < t) {
            into[t].push(s);
            last[s] = Some(last[s].map_or(t, |l| l.max(t)));
        }
    }
    // Which row holds each lane, as each row is reached, and where each row's dot is.
    let mut held: Vec<Option<usize>> = vec![];
    let mut seen: Vec<Vec<(usize, usize)>> = vec![];
    let mut dot = vec![0; n];
    for r in 0..n {
        seen.push(held.iter().enumerate().filter_map(|(l, o)| o.map(|o| (l, o))).collect());
        // Not on a lane that ends here: a wire joins its dot from the side, so it shows (a row
        // straight under the one it takes from would otherwise look unjoined).
        let d = held.iter().position(Option::is_none).unwrap_or(held.len());
        dot[r] = d;
        for o in held.iter_mut() {
            if o.is_some_and(|o| last[o] == Some(r)) {
                *o = None;
            }
        }
        if d == held.len() {
            held.push(None);
        }
        if last[r].is_some() {
            held[d] = Some(r);
        }
    }
    let width = held.len().max(1) * 2 - 1;
    (0..n)
        .map(|r| {
            let mut cells = vec![' '; width];
            let d = dot[r] * 2;
            let joins: Vec<usize> = seen[r].iter().filter(|(_, o)| into[r].contains(o)).map(|(l, _)| l * 2).collect();
            let lo = joins.iter().copied().chain([d]).min().unwrap_or(d);
            let hi = joins.iter().copied().chain([d]).max().unwrap_or(d);
            cells[lo..=hi].fill('─');
            for &(l, o) in &seen[r] {
                let x = l * 2;
                let across = x > lo && x <= hi || x < hi && x >= lo;
                cells[x] = joint(true, last[o].is_some_and(|e| e > r), across && x > lo, across && x < hi);
            }
            cells[d] = '●';
            cells.into_iter().collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{graph, lanes};

    const PAYOUT: &str = include_str!("../samples/approved-payout.holon.ts");

    #[test]
    fn a_flow_is_its_statements_by_trigger_with_the_nodes_under_them() {
        let g = graph(PAYOUT, false).unwrap();
        let titles: Vec<&str> = g.bands.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, ["POST /payout/:account", "action payout", "POST /approve/:token", "POST /deposit/:account", "GET /accounts/:account"]);
        let route = &g.bands[0];
        let names: Vec<&str> = route.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["POST /payout/:account", "account", "started", "return"]);
        assert_eq!(route.rows[0].does, "request");
        assert_eq!(route.rows[2].does, "flow_start");
        let payout = &g.bands[1];
        let names: Vec<&str> = payout.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["action payout", "account", "approved", "sent", "bank.exec", "bank.exec"]);
        assert_eq!(payout.rows[1].does, "db_one");
        // `sent` takes from what was asked, the account and the approval; not from the plumbing.
        assert_eq!(payout.rows[3].from, [0, 1, 2]);
        assert!(payout.rows[1].parts.iter().any(|p| p.leaf == "field"), "its plumbing is its detail");
        assert!(payout.rows.iter().all(|r| r.line > 0 && r.last >= r.line));
        assert!(g.nodes > payout.rows.len() * 3, "far fewer rows than nodes");

        let every = graph(PAYOUT, true).unwrap();
        assert_eq!(every.bands[1].rows.len(), every.bands[1].rows.iter().skip(1).map(|r| r.parts.len()).sum::<usize>() + 1);
    }

    #[test]
    fn a_row_marks_where_the_text_says_it() {
        let text = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"bump\")\n\nexport const bump = action(() => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  return n\n})\n";
        let lines: Vec<&str> = text.lines().collect();
        let said = |s: crate::lang::ast::Span| -> String {
            assert_eq!(s.start.line, s.end.line, "{s:?}");
            lines[s.start.line as usize - 1].chars().skip(s.start.col as usize - 1).take((s.end.col - s.start.col) as usize).collect()
        };
        let g = graph(text, false).unwrap();
        let marks: Vec<String> = g.bands[0].rows.iter().map(|r| said(r.mark)).collect();
        assert_eq!(marks, ["export const bump = action(() => {", "n", "state.n", "return"]);
        // Every node: each its own code, on its statement's line.
        let every = graph(text, true).unwrap();
        for r in every.bands[0].rows.iter().skip(1) {
            assert!((r.line..=r.last).contains(&r.mark.start.line) && !said(r.mark).is_empty(), "{r:?}");
        }
        assert!(every.bands[0].rows.iter().any(|r| said(r.mark) == "state.n"), "{:?}", every.bands[0].rows.iter().map(|r| said(r.mark)).collect::<Vec<_>>());
    }

    #[test]
    fn lanes_run_from_a_row_down_to_the_last_row_it_feeds() {
        let g = graph(PAYOUT, false).unwrap();
        let drawn = lanes(&g.bands[0].rows);
        let width = drawn[0].chars().count();
        assert!(drawn.iter().all(|l| l.chars().count() == width), "{drawn:#?}");
        assert!(drawn.iter().all(|l| l.contains('●')), "{drawn:#?}");
        assert_eq!(drawn[0].trim_end(), "●");
        assert!(drawn[1].starts_with("├─●"), "{drawn:#?}");
        // The last row takes the lanes still open into it: none goes on below.
        assert!(!drawn.last().unwrap().contains(['│', '├', '┼']), "{drawn:#?}");
    }

    #[test]
    fn a_clock_is_a_band() {
        let g = graph("import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"beats\")\n\nevery(\"5s\", () => {\n  state.beats = (state.beats ?? 0) + 1\n})\n", false).unwrap();
        assert_eq!(g.bands.len(), 1);
        assert_eq!(g.bands[0].title, "every 5s");
        assert_eq!(g.bands[0].rows[1].name, "state.beats");
        assert!(g.bands[0].rows[1].effect);
    }

    /// Every sample: its handlers named as the log names their runs, each trigger's row its
    /// trigger alone, every other node under a statement.
    #[test]
    fn every_samples_handlers_are_named_as_their_runs_are() {
        for s in crate::samples::samples() {
            let g = graph(s.text, false).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            let heads: Vec<(&str, usize)> = g.bands.iter().map(|b| (b.title.as_str(), b.rows[0].parts.len())).collect();
            assert!(heads.iter().all(|(title, parts)| *parts == 1 && !title.contains("=>")), "{}: {heads:?}", s.name);
            assert!(g.bands.iter().flat_map(|b| &b.rows).all(|r| r.line > 0 && r.last >= r.line), "{}", s.name);
        }
        let titles: Vec<String> = graph(crate::samples::sample("file-audit").unwrap().text, false).unwrap().bands.into_iter().map(|b| b.title).collect();
        assert_eq!(titles, ["on file inbox matching \"*.csv\"", "on file inbox deleted", "on file outbox"]);
    }
}
