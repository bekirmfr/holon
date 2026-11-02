//! A flow's graph, as `holon editor` shows it: on a deployment's page (its Graph section, over
//! the text deployed, with what its latest runs reached) and in an editor tab (over the text as
//! typed). Its statements by trigger, each a row with its wires drawn as lanes beside it
//! (holon_author::graph); the picked row's text, wires and nodes on the right.

use std::collections::HashSet;

use holon_author::graph::{self, Graph};
use serde_json::Value;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::app::{App, List, Show, Tone};
use super::client::{Client, Detail};
use super::flow::{at_ms, failed, Row};

/// A graph as drawn: its rows in order, each with its band and its lanes.
pub(crate) struct GraphView {
    /// What it was built from, to tell when to build it again.
    pub(crate) key: String,
    pub(crate) graph: Graph,
    /// The text's lines.
    pub(crate) text: Vec<String>,
    /// Every row: its band, its index in the band, its lanes (padded to one width).
    pub(crate) rows: Vec<(usize, usize, String)>,
    /// The lines the newest run of each trigger reached, when it ran on this text.
    pub(crate) reached: HashSet<u32>,
    /// Why there is no graph: the text doesn't compile.
    pub(crate) error: Option<String>,
    /// Every node a row, not only the statements.
    pub(crate) every: bool,
    /// Over a run: what it didn't reach is dimmed.
    pub(crate) dim: bool,
}

/// An editor tab's graph: over its text as typed, or over the text a run ran on.
pub(crate) struct TabGraph {
    pub(crate) view: GraphView,
    pub(crate) sel: usize,
    /// The right pane has the arrows: they scroll it.
    pub(crate) right: bool,
    pub(crate) scroll: usize,
}

impl GraphView {
    pub(crate) fn build(key: String, text: &str, every: bool) -> GraphView {
        let lines = text.lines().map(str::to_string).collect();
        let (graph, error) = match graph::graph(text, every) {
            Ok(g) => (g, None),
            Err(e) => (Graph::default(), Some(e)),
        };
        let drawn: Vec<Vec<String>> = graph.bands.iter().map(|b| graph::lanes(&b.rows)).collect();
        let width = drawn.iter().flatten().map(|l| l.chars().count()).max().unwrap_or(1);
        let mut rows = vec![];
        for (b, lanes) in drawn.into_iter().enumerate() {
            for (r, l) in lanes.into_iter().enumerate() {
                let pad = width - l.chars().count();
                rows.push((b, r, format!("{l}{}", " ".repeat(pad))));
            }
        }
        GraphView { key, graph, text: lines, rows, reached: HashSet::new(), error, every, dim: false }
    }

    pub(crate) fn row(&self, i: usize) -> Option<&graph::Row> {
        let (b, r, _) = self.rows.get(i)?;
        self.graph.bands.get(*b)?.rows.get(*r)
    }

    /// The (first) row of the statement a line of the text is in, else the nearest before it.
    pub(crate) fn row_at(&self, line: u32) -> usize {
        let at = |i: usize| self.row(i).map(|r| (r.line, r.last));
        (0..self.rows.len())
            .find(|&i| at(i).is_some_and(|(a, b)| a <= line && line <= b))
            .or_else(|| (0..self.rows.len()).rev().find(|&i| at(i).is_some_and(|(a, _)| a <= line)))
            .unwrap_or(0)
    }

    /// The lines of every statement a line of `reached` is in: what a run played, whole.
    pub(crate) fn lit(&self) -> HashSet<u32> {
        let mut lit = self.reached.clone();
        for i in 0..self.rows.len() {
            if let Some(r) = self.row(i).filter(|r| (r.line..=r.last).any(|n| self.reached.contains(&n))) {
                lit.extend(r.line..=r.last);
            }
        }
        lit
    }

    /// A row's part of a run: its text with the values there, and when it ran.
    pub(crate) fn run_pane(&self, i: usize, v: &holon_author::agent::RunView) -> Vec<Row> {
        let Some(row) = self.row(i) else { return vec![Row::Text(Tone::Dim, "Nothing here.".into())] };
        let band = &self.graph.bands[self.rows[i].0];
        let mut rows = vec![Row::Text(Tone::Head, if row.head { band.title.clone() } else { format!("{}  ({})", row.name, band.title) })];
        if !(row.line..=row.last).any(|n| self.reached.contains(&n)) {
            rows.push(Row::Text(Tone::Dim, "This run didn't play it.".into()));
        }
        rows.extend((row.line..=row.last).map(|n| Row::Code { n, text: self.text.get(n as usize - 1).cloned().unwrap_or_default(), notes: v.notes.get(&n).cloned().unwrap_or_default() }));
        if let Some(s) = v.steps.iter().find(|s| (row.line..=row.last).contains(&s.line)) {
            rows.push(Row::Field("Ran".into(), format!("from {} to {}", super::flow::took(s.start_us / 1000.0), super::flow::took(s.end_us / 1000.0)), Tone::Plain));
        }
        if !row.does.is_empty() {
            rows.push(Row::Field("Is".into(), row.does.clone(), Tone::Plain));
        }
        rows
    }

    /// Mark what the newest run of each trigger reached, when it ran on this version.
    pub(crate) fn reach(&mut self, c: &Client, d: &Detail) {
        self.reached.clear();
        for band in &self.graph.bands {
            let Some(r) = newest_run(d, &band.title) else { continue };
            if r["unit"] != d.entry["unit"] {
                continue;
            }
            if let Ok(v) = c.run_view(&d.slug, r) {
                self.reached.extend(v.notes.keys());
            }
        }
    }

    /// The right pane for row `i`: its text, wires and nodes (tab 0), or what the newest run of
    /// its trigger gave there (tab 1, with a deployment).
    pub(crate) fn pane(&self, i: usize, tab: usize, run: Option<(&Client, &Detail)>) -> Vec<Row> {
        let text = |t: Tone, s: String| Row::Text(t, s);
        if let Some(e) = &self.error {
            return vec![text(Tone::Bad, "It doesn't compile, so it has no graph yet:".into()), text(Tone::Plain, e.clone())];
        }
        let Some(row) = self.row(i) else { return vec![text(Tone::Dim, "Nothing here.".into())] };
        let (b, r, _) = self.rows[i];
        let band = &self.graph.bands[b];
        let code = |notes: &dyn Fn(u32) -> Vec<String>| -> Vec<Row> {
            (row.line..=row.last).map(|n| Row::Code { n, text: self.text.get(n as usize - 1).cloned().unwrap_or_default(), notes: notes(n) }).collect()
        };
        if tab == 1 {
            let Some((c, d)) = run else { return vec![text(Tone::Dim, "Runs are a deployment's: deploy it to see them here.".into())] };
            let Some(entry) = newest_run(d, &band.title) else {
                return vec![text(Tone::Dim, format!("Nothing has run {} yet.", band.title)), text(Tone::Dim, "c calls it.".into())];
            };
            let seq = entry["seq"].as_u64().unwrap_or_default();
            let mut rows = vec![Row::Field("Newest run".into(), format!("#{seq}, {}", at_ms(&entry["at_ms"])), if failed(entry) { Tone::Bad } else { Tone::Plain })];
            if entry["unit"] != d.entry["unit"] {
                rows.push(text(Tone::Dim, "It ran on an earlier version, whose text differs: Runs shows it over that text.".into()));
                return rows;
            }
            match c.run_view(&d.slug, entry) {
                Ok(v) => {
                    if !(row.line..=row.last).any(|n| v.notes.contains_key(&n)) {
                        rows.push(text(Tone::Dim, "It recorded no value here: Runs shows the whole run over its text.".into()));
                    }
                    rows.extend(code(&|n| v.notes.get(&n).cloned().unwrap_or_default()));
                    if let Some(e) = entry["error"].as_str() {
                        rows.push(Row::Field("Failed".into(), e.lines().next().unwrap_or_default().to_string(), Tone::Bad));
                    }
                }
                Err(e) => rows.push(text(Tone::Bad, e)),
            }
            return rows;
        }
        let mut rows = vec![text(Tone::Head, if row.head { band.title.clone() } else { format!("{}  ({})", row.name, band.title) })];
        rows.extend(code(&|_| vec![]));
        if !row.does.is_empty() {
            rows.push(Row::Field("Is".into(), row.does.clone(), Tone::Plain));
        }
        let name = |j: usize| band.rows.get(j).map(|o| if o.head { "the trigger".to_string() } else { format!("{} (line {})", o.name, o.line) }).unwrap_or_default();
        let mut from: Vec<String> = row.from.iter().map(|&j| name(j)).collect();
        from.extend(row.outside.iter().map(|o| format!("{o}, outside this trigger")));
        if !from.is_empty() {
            rows.push(Row::Field("Takes from".into(), from.join(", "), Tone::Plain));
        }
        let feeds: Vec<String> = band.rows.iter().enumerate().filter(|(_, o)| o.from.contains(&r)).map(|(j, _)| name(j)).collect();
        if !feeds.is_empty() {
            rows.push(Row::Field("Feeds".into(), feeds.join(", "), Tone::Plain));
        }
        if !row.parts.is_empty() {
            rows.push(text(Tone::Head, format!("Made of {} node{}", row.parts.len(), if row.parts.len() == 1 { "" } else { "s" })));
            for p in &row.parts {
                let params: Vec<String> = p.params.iter().map(|(k, v)| format!("{k}={}", Value::String(v.clone()))).collect();
                rows.push(text(Tone::Plain, format!("{}  {}  {}", p.name, p.leaf, params.join(" ")).trim_end().to_string()));
                rows.extend(p.inputs.iter().map(|(port, src)| text(Tone::Dim, format!("    {port} ← {src}"))));
            }
        }
        rows
    }
}

impl App {
    /// Code, then code and graph, then the graph alone, then code again.
    pub(crate) fn next_show(&mut self) {
        self.set_show(self.show.next());
    }

    /// Show the tabs' text, graph, or both: the graph alone takes the keys; else, unless a run
    /// is shown, the text does.
    pub(crate) fn set_show(&mut self, to: Show) {
        let from = self.show;
        self.show = to;
        self.complete = None;
        self.graph_keys = false;
        self.sync_graph();
        let runs = self.tabs.get(self.tab).is_some_and(|t| t.runs.is_some());
        if let Some(tab) = self.tabs.get_mut(self.tab)
            && let Some(g) = tab.graph.as_mut()
            && to == Show::Graph
            && from != Show::Graph
            && !runs
        {
            (g.sel, g.right, g.scroll) = (g.view.row_at(tab.buf.row as u32 + 1), false, 0);
        }
        self.typing = to != Show::Graph && !runs;
        let what = match to {
            Show::Code => "the code",
            Show::Both => "the code, its graph beside it",
            Show::Graph => "its graph: enter edits the statement picked",
        };
        let next = match to.next() {
            Show::Code => "the code alone",
            Show::Both => "code and graph",
            Show::Graph => "the graph alone",
        };
        self.say(format!("{what} (ctrl+b: {next})"));
    }

    /// The tab's graph as it should be now: built (again) from its text as typed, or from the
    /// text its picked run ran on, with what that run played lit, at the step picked or the
    /// statement the cursor is in.
    pub(crate) fn sync_graph(&mut self) {
        let show = self.show;
        let beside = self.beside_has_keys();
        let Some(tab) = self.tabs.get_mut(self.tab) else { return };
        let run = tab.runs.as_ref().map(|r| (r.view.as_ref().and_then(|v| v.as_ref().ok()).map(|v| v.text.join("\n")), r.at(), r.steps()));
        if show == Show::Code && run.is_none() {
            return;
        }
        let text = match &run {
            Some((Some(t), ..)) => t.clone(),
            Some((None, ..)) => {
                tab.graph = None;
                return;
            }
            None => tab.buf.text(),
        };
        // Every node a row, until a says the statements.
        let every = tab.graph.as_ref().is_none_or(|g| g.view.every);
        let key = format!("{every}\n{text}");
        if tab.graph.as_ref().is_none_or(|g| g.view.key != key) {
            let line = tab.graph.as_ref().and_then(|g| g.view.row(g.sel)).map_or(tab.buf.row as u32 + 1, |r| r.line);
            let view = GraphView::build(key, &text, every);
            let sel = view.row_at(line);
            let right = tab.graph.as_ref().is_some_and(|g| g.right);
            tab.graph = Some(TabGraph { view, sel, right, scroll: 0 });
        }
        let Some(g) = tab.graph.as_mut() else { return };
        match run {
            Some((_, at, steps)) => {
                g.view.reached = steps.into_iter().collect();
                g.view.dim = true;
                if let Some(sel) = at.map(|at| g.view.row_at(at)).filter(|s| *s != g.sel) {
                    (g.sel, g.scroll) = (sel, 0);
                }
            }
            None => {
                g.view.reached.clear();
                g.view.dim = false;
                // Beside the text, the graph follows the cursor, unless it has the keys.
                if show == Show::Both && !beside {
                    g.sel = g.view.row_at(tab.buf.row as u32 + 1);
                }
            }
        }
    }

    /// Leave the graph alone for the text: at the picked statement, the graph beside it; or
    /// back to the code alone, where the cursor was.
    fn graph_to_text(&mut self, at_pick: bool) {
        let Some(tab) = self.tabs.get_mut(self.tab) else { return };
        if at_pick && let Some(m) = tab.graph.as_ref().and_then(|g| g.view.row(g.sel)).map(|r| r.mark) {
            tab.buf.go(m.start.line, m.start.col);
            return self.set_show(Show::Both);
        }
        self.set_show(Show::Code);
    }

    /// Code and graph side by side: the graph beside the text has the keys (no run shown).
    pub(crate) fn beside_has_keys(&self) -> bool {
        self.page == super::app::Page::Editor
            && self.show == Show::Both
            && self.graph_keys
            && !self.typing
            && self.modal.is_none()
            && self.tabs.get(self.tab).is_some_and(|t| t.runs.is_none() && t.graph.is_some())
    }

    /// Beside the text, the graph takes the keys: ↑↓ pick its rows, the text marking the one
    /// picked, the cursor staying where it was; esc gives them back.
    pub(crate) fn graph_beside(&mut self) {
        self.sync_graph();
        if self.tabs.get(self.tab).is_none_or(|t| t.graph.is_none() || t.runs.is_some()) {
            return;
        }
        (self.graph_keys, self.typing, self.complete, self.find) = (true, false, None, None);
        self.say("the graph has the keys: ↑↓ pick, marked in the text; enter edits it there, esc goes back to the text");
    }

    /// The row picked in the graph beside the text, edited: the cursor at its mark, the text
    /// taking the keys again.
    fn beside_edit(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.tab) else { return };
        if let Some(m) = tab.graph.as_ref().and_then(|g| g.view.row(g.sel)).map(|r| r.mark) {
            tab.buf.go(m.start.line, m.start.col);
        }
        (self.graph_keys, self.typing) = (false, true);
    }

    /// The graph beside the text, with the keys: the graph's own, as it has them alone.
    pub(crate) fn beside_key(&mut self, k: KeyEvent) {
        let Some(g) = self.tabs.get_mut(self.tab).and_then(|t| t.graph.as_mut()) else { return };
        let n = g.view.rows.len();
        let to = |at: usize, by: isize| (at as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        match k.code {
            KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => (self.graph_keys, self.typing) = (false, true),
            KeyCode::Enter => self.beside_edit(),
            KeyCode::Char('a') => self.tab_every(),
            KeyCode::Char('b') => self.next_show(),
            // Its detail: the graph alone, at the same row, its right pane open.
            KeyCode::Right | KeyCode::Char('l') => {
                let sel = g.sel;
                self.set_show(Show::Graph);
                if let Some(g) = self.tabs.get_mut(self.tab).and_then(|t| t.graph.as_mut()) {
                    (g.sel, g.right, g.scroll) = (sel, true, 0);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => g.sel = to(g.sel, -1),
            KeyCode::Down | KeyCode::Char('j') => g.sel = to(g.sel, 1),
            KeyCode::PageUp => g.sel = to(g.sel, -10),
            KeyCode::PageDown => g.sel = to(g.sel, 10),
            KeyCode::Home => g.sel = 0,
            KeyCode::End => g.sel = n.saturating_sub(1),
            _ => {}
        }
    }

    /// A tab's graph: every node a row, or the statements with their nodes under each; the
    /// same statement stays picked.
    pub(crate) fn tab_every(&mut self) {
        self.sync_graph();
        let Some(g) = self.tabs.get_mut(self.tab).and_then(|t| t.graph.as_mut()) else { return };
        let line = g.view.row(g.sel).map(|r| r.line).unwrap_or(1);
        let text = g.view.text.join("\n");
        let (reached, dim) = (std::mem::take(&mut g.view.reached), g.view.dim);
        g.view = GraphView::build(format!("{}\n{text}", !g.view.every), &text, !g.view.every);
        (g.view.reached, g.view.dim) = (reached, dim);
        g.sel = g.view.row_at(line);
        g.scroll = 0;
    }

    pub(crate) fn tab_graph_key(&mut self, k: KeyEvent) {
        self.sync_graph();
        let Some(g) = self.tabs.get_mut(self.tab).and_then(|t| t.graph.as_mut()) else {
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('b')) {
                self.next_show();
            }
            return;
        };
        let n = g.view.rows.len();
        let to = |at: usize, by: isize, n: usize| (at as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        match k.code {
            KeyCode::Esc if g.right => g.right = false,
            KeyCode::Esc => self.graph_to_text(false),
            KeyCode::Char('b') => self.next_show(),
            KeyCode::Enter => self.graph_to_text(true),
            KeyCode::Right | KeyCode::Char('l') => g.right = true,
            KeyCode::Left | KeyCode::Char('h') => g.right = false,
            KeyCode::Char('a') => self.tab_every(),
            KeyCode::Up | KeyCode::Char('k') if g.right => g.scroll = g.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if g.right => g.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => (g.sel, g.scroll) = (to(g.sel, -1, n), 0),
            KeyCode::Down | KeyCode::Char('j') => (g.sel, g.scroll) = (to(g.sel, 1, n), 0),
            KeyCode::PageUp => (g.sel, g.scroll) = (to(g.sel, -10, n), 0),
            KeyCode::PageDown => (g.sel, g.scroll) = (to(g.sel, 10, n), 0),
            KeyCode::Home => (g.sel, g.scroll) = (0, 0),
            KeyCode::End => (g.sel, g.scroll) = (n.saturating_sub(1), 0),
            _ => {}
        }
    }

    /// A row clicked: over a run, the step there (a row it didn't play is only picked); beside
    /// the text, it is picked, the graph taking the keys; alone, it is picked. A second click
    /// edits it there.
    pub(crate) fn tab_graph_pick(&mut self, list: List, i: usize, again: bool) {
        let show = self.show;
        let Some(tab) = self.tabs.get_mut(self.tab) else { return };
        let Some(g) = tab.graph.as_mut() else { return };
        if list == List::GraphPane {
            g.right = true;
            return;
        }
        let i = i.min(g.view.rows.len().saturating_sub(1));
        let span = g.view.row(i).map(|r| (r.line, r.last));
        if let Some(runs) = tab.runs.as_mut() {
            match span.and_then(|(a, b)| runs.steps().iter().position(|n| (a..=b).contains(n))) {
                Some(step) => runs.step = step,
                None => (g.sel, g.scroll) = (i, 0),
            }
            return;
        }
        if show == Show::Both {
            (g.sel, g.scroll) = (i, 0);
            (self.graph_keys, self.typing) = (true, false);
            if again {
                self.beside_edit();
            }
            return;
        }
        (g.sel, g.right, g.scroll) = (i, false, 0);
        if again {
            self.graph_to_text(true);
        }
    }

    /// The wheel over the graph: over a run, the steps; beside the text, the cursor by
    /// statements (the pick, when the graph has the keys); alone, the pick.
    pub(crate) fn tab_graph_scroll(&mut self, list: List, by: isize) {
        let show = self.show;
        let beside = self.beside_has_keys();
        let Some(tab) = self.tabs.get_mut(self.tab) else { return };
        if let Some(runs) = tab.runs.as_mut().filter(|_| list == List::Graph) {
            return runs.go(by.signum());
        }
        let Some(g) = tab.graph.as_mut() else { return };
        if list == List::GraphPane {
            g.scroll = (g.scroll as isize + by).max(0) as usize;
            return;
        }
        g.sel = (g.sel as isize + by).clamp(0, g.view.rows.len().saturating_sub(1) as isize) as usize;
        g.scroll = 0;
        if show == Show::Both
            && !beside
            && let Some(line) = g.view.row(g.sel).map(|r| r.line)
        {
            tab.buf.go(line, 1);
        }
    }

    /// Open a deployment's text in the editor, at `line`.
    pub(crate) fn edit_at(&mut self, slug: &str, line: u32) {
        self.edit(slug);
        if let Some(tab) = self.tabs.get_mut(self.tab).filter(|t| t.slug.as_deref() == Some(slug)) {
            tab.runs = None;
            tab.buf.go(line, 1);
            if self.show == Show::Graph {
                self.show = Show::Both;
            }
            self.typing = true;
        }
    }
}

/// The newest run of the trigger titled `title` (`route POST /notes/:id (req)`, `every 5s`):
/// runs name their route as `POST /notes/:id`.
fn newest_run<'a>(d: &'a Detail, title: &str) -> Option<&'a Value> {
    d.runs.iter().rev().find(|r| r["route"].as_str().is_some_and(|route| !route.is_empty() && title.contains(route)))
}
