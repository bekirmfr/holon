//! A deployment's page: a head with its state and buttons, the sections, then the dashboard, or
//! a list on the left with the picked row's tabs on the right.

use std::time::{SystemTime, UNIX_EPOCH};

use holon_author::agent;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use serde_json::Value;

use super::{bold, chips, coloured, dim, fit, list, pad, panel, reach, scrollbar, status, tile, tone, ACCENT, BAD, DARK, DIM, GOOD, WARN};
use crate::workbench::app::{App, Click, List, Tone};
use crate::workbench::client::{Detail, Moment};
use crate::workbench::flow::{access_rows, at_ms, came_in, failed, tabs, took, Access, Held, Row, Section, SECTIONS};
use crate::workbench::graph::GraphView;

pub(super) fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(d) = app.detail.as_ref() else { return };
    let attention = d.entry["attention"].as_str().or(d.entry["listen_error"].as_str()).map(str::to_string);
    let [head, warn, bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(if attention.is_some() { 1 } else { 0 }), Constraint::Length(2), Constraint::Min(3)]).areas(area);
    draw_head(f, app, d, head);
    if let Some(a) = attention {
        f.render_widget(Paragraph::new(coloured(format!(" ! {a}"), WARN)), warn);
    }
    draw_sections(f, app, d, bar);
    if app.flow.section == Section::Status {
        dashboard(f, app, d, body);
    } else {
        split(f, app, body);
    }
}

fn draw_head(f: &mut Frame, app: &App, d: &Detail, area: Rect) {
    let e = &d.entry;
    let unit = e["unit"].as_str().unwrap_or("");
    let version = d.versions.iter().find(|v| v["current"] == true).and_then(|v| v["number"].as_u64()).map(|n| format!("v{n}")).unwrap_or_default();
    let by = e["deployed_by"].as_str().map(|b| format!(" by {b}")).unwrap_or_default();
    let line = Line::from(vec![
        Span::styled(format!(" {} ", d.slug), Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        status(e),
        dim(format!("   {version} {}   {}   deployed{by}, {}", &unit[..unit.len().min(8)], reach(e), crate::mcp::when(&e["deployed_at_epoch_seconds"]))),
    ]);
    f.render_widget(Paragraph::new(line), area);
    // The buttons, on the right.
    use KeyCode::Char;
    let running = e["status"] == "running";
    let mut keys = vec![("e", "edit", Char('e'), KeyModifiers::NONE), ("c", "call", Char('c'), KeyModifiers::NONE)];
    if running {
        keys.push(("p", "pause", Char('p'), KeyModifiers::NONE));
        keys.push(("x", "stop", Char('x'), KeyModifiers::NONE));
    } else {
        keys.push(("g", "run", Char('g'), KeyModifiers::NONE));
    }
    let width: u16 = keys.iter().map(|(k, w, ..)| (k.chars().count() + w.chars().count() + 4) as u16).sum::<u16>() + 1;
    if area.width > width + 60 {
        chips(f, app, Rect { x: area.x + area.width - width, width, ..area }, &keys);
    }
}

fn draw_sections(f: &mut Frame, app: &App, d: &Detail, area: Rect) {
    let counts = [None, Some(d.runs.len()), Some(d.moments().len()), Some(app.flow.cells.len()), None, None];
    let mut x = area.x + 1;
    for ((s, name), n) in SECTIONS.iter().zip(counts) {
        let here = app.flow.section == *s;
        let label = match n {
            Some(n) => format!(" {name} {n} "),
            None => format!(" {name} "),
        };
        let w = label.chars().count() as u16;
        if x + w > area.x + area.width {
            break;
        }
        let style = if here { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
        let rect = Rect { x, y: area.y, width: w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        f.render_widget(Paragraph::new(Span::styled(if here { "━".repeat(w as usize) } else { String::new() }, Style::new().fg(ACCENT))), Rect { y: area.y + 1, ..rect });
        app.hit(Rect { height: 2, ..rect }, Click::Section(*s));
        x += w + 1;
    }
    let rule = Rect { x, y: area.y + 1, width: (area.x + area.width).saturating_sub(x), height: 1 };
    f.render_widget(Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::new().fg(DIM))), rule);
    let first = Rect { x: area.x, y: area.y + 1, width: 1, height: 1 };
    f.render_widget(Paragraph::new(Span::styled("─", Style::new().fg(DIM))), first);
}

// ── the dashboard ────────────────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

/// How long ago, in a word or two.
fn ago(ms: u64) -> String {
    let s = now_ms().saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    (!sorted.is_empty()).then(|| sorted[((sorted.len() - 1) as f64 * p).round() as usize])
}

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The last 24 hours, an hour a bar: how many runs, red where some failed.
fn sparkline(d: &Detail, width: usize) -> (Line<'static>, usize, usize) {
    let now = now_ms();
    let mut hours = [(0usize, 0usize); 24];
    for r in &d.runs {
        let Some(at) = r["at_ms"].as_u64() else { continue };
        let back = now.saturating_sub(at) / 3_600_000;
        if back < 24 {
            let h = &mut hours[23 - back as usize];
            h.0 += 1;
            h.1 += failed(r) as usize;
        }
    }
    let top = hours.iter().map(|h| h.0).max().unwrap_or(0).max(1);
    let each = (width / 24).max(1);
    let mut spans = vec![];
    for (n, bad) in hours {
        let c = if n == 0 { '·' } else { BARS[((n * 7) / top).min(7)] };
        let colour = if bad > 0 { BAD } else if n == 0 { DIM } else { ACCENT };
        spans.push(Span::styled(c.to_string().repeat(each.saturating_sub(1).max(1)), Style::new().fg(colour)));
        if each > 1 {
            spans.push(Span::raw(" "));
        }
    }
    (Line::from(spans), hours.iter().map(|h| h.0).sum(), hours.iter().map(|h| h.1).sum())
}

fn dashboard(f: &mut Frame, app: &App, d: &Detail, area: Rect) {
    let e = &d.entry;
    let runs = e["trigger_count"].as_u64().unwrap_or(0);
    let errors = e["error_count"].as_u64().unwrap_or(0);
    let mut times: Vec<f64> = d.runs.iter().filter_map(|r| r["duration_ms"].as_f64()).collect();
    times.sort_by(f64::total_cmp);
    let last = d.runs.last();
    let wide = area.width >= 96;

    let [tiles, rest] = Layout::vertical([Constraint::Length(if area.height >= 16 { 5 } else { 0 }), Constraint::Min(3)]).areas(area);
    if tiles.height > 0 {
        let parts = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(tiles);
        tile(f, parts[0], "Runs", &runs.to_string(), &format!("session {} of {}", e["session"], e["sessions"]), Color::White);
        let rate = if runs > 0 { format!("{:.1}% of runs", errors as f64 * 100.0 / runs as f64) } else { "no runs yet".into() };
        tile(f, parts[1], "Failed", &errors.to_string(), &rate, if errors > 0 { BAD } else { GOOD });
        let p50 = percentile(&times, 0.5).map(took).unwrap_or_else(|| "–".into());
        let p95 = percentile(&times, 0.95).map(|t| format!("p95 {}", took(t))).unwrap_or_default();
        tile(f, parts[2], "Typical run (p50)", &p50, &p95, Color::White);
        let (when, what, colour) = match last {
            Some(r) => (r["at_ms"].as_u64().map(ago).unwrap_or_default(), came_in(r), if failed(r) { BAD } else { GOOD }),
            None => ("never".into(), "call it: c".into(), DIM),
        };
        tile(f, parts[3], "Last run", &when, &what, colour);
    }
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(rest) } else { [rest, Rect::default()] };

    // Left: the last day, then what it may do.
    let may = crate::workbench::client::strings(&d.manifest, "items");
    // What it holds, in memory and on disk (storage.rs), below what it may do.
    let holds: Vec<String> = if d.holds.is_object() { crate::storage::holds_lines(&d.holds) } else { vec![] };
    let holds_h = if holds.is_empty() { 0 } else { holds.len() as u16 + 2 };
    let [day, may_area, holds_area] = Layout::vertical([Constraint::Length(5), Constraint::Min(3), Constraint::Length(holds_h)]).areas(left);
    if holds_h > 0 {
        f.render_widget(Paragraph::new(holds.into_iter().map(|l| Line::from(Span::raw(l))).collect::<Vec<_>>()).block(panel("It holds", false)), holds_area);
    }
    let block = panel("Last 24 h", false);
    let inner = block.inner(day);
    f.render_widget(block, day);
    let (spark, n, bad) = sparkline(d, inner.width as usize);
    let in_view = if d.runs.len() as u64 >= crate::workbench::client::RUNS { " (of the newest runs read)" } else { "" };
    f.render_widget(
        Paragraph::new(vec![spark, Line::from(vec![dim("runs "), bold(n.to_string()), dim("   failed "), Span::styled(bad.to_string(), Style::new().fg(if bad > 0 { BAD } else { GOOD }).add_modifier(Modifier::BOLD)), dim(format!("   an hour a bar, now on the right{in_view}"))])]),
        inner,
    );
    let not_granted = crate::workbench::client::strings(&d.manifest, "not_granted");
    let mut lines: Vec<Line> = if may.is_empty() { vec![Line::from(dim("only compute"))] } else { vec![] };
    for i in &may {
        let missing = not_granted.contains(i);
        lines.push(Line::from(vec![if missing { coloured("✗ ", BAD) } else { coloured("✓ ", GOOD) }, Span::raw(agent::describe(i)), if missing { coloured("  its grant doesn't cover this", BAD) } else { Span::raw("") }]));
    }
    let reads = crate::secretcmd::reads(&d.manifest);
    let set: Vec<&str> = d.secrets["secrets"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).collect();
    for r in &reads {
        if !set.contains(&r.as_str()) {
            lines.push(Line::from(vec![coloured("! ", WARN), Span::raw(format!("reads the secret {r}, which isn't set")), dim("  (Access sets it)")]));
        }
    }
    f.render_widget(Paragraph::new(lines).block(panel("It may", false)), may_area);
    app.hit(may_area, Click::Section(Section::Access));
    if !wide {
        return;
    }

    // Right: what waits, its versions (enter goes back to one), its latest runs.
    let waits = crate::mcp::waits_for(&d.entry, app.client.as_ref().and_then(|c| c.saved(&d.slug)));
    let wait_h = if waits.is_empty() && d.parked.is_empty() { 0 } else { (waits.len() + d.parked.len()).min(4) as u16 + 2 };
    let versions_h = (d.versions.len().clamp(1, 6) + 2) as u16;
    let [wait_area, versions, latest] = Layout::vertical([Constraint::Length(wait_h), Constraint::Length(versions_h), Constraint::Min(3)]).areas(right);
    if wait_h > 0 {
        let mut lines: Vec<Line> = waits.iter().map(|w| Line::from(vec![coloured("● ", WARN), Span::raw(w.clone())])).collect();
        lines.extend(d.parked.iter().map(|r| Line::from(vec![coloured("◷ ", WARN), Span::raw(format!("a run waits: {} {}", r["kind"].as_str().unwrap_or_default(), r["route"].as_str().unwrap_or_default()))])));
        f.render_widget(Paragraph::new(lines).block(panel("Waiting", false)), wait_area);
    }
    let block = panel(&format!("Versions ({})", d.versions.len()), true);
    super::keys_here(app, versions);
    let inner = block.inner(versions);
    f.render_widget(block, versions);
    list(f, app, inner, List::Section, d.versions.len(), Some(app.flow.sel()), true, |i, w| {
        let v = &d.versions[i];
        let now = v["current"] == true;
        let at = v["deployed_at"].as_i64().map(|s| crate::engines::time_text(s * 1_000_000).replace('T', " ")).unwrap_or_default();
        let note = if now {
            coloured("live   ", GOOD)
        } else if v["restorable"] == true {
            dim("       ")
        } else {
            dim("old    ")
        };
        let mut tail = v["by"].as_str().map(|b| format!("by {b}")).unwrap_or_default();
        if let Some(from) = v["restored_from"].as_u64() {
            tail.push_str(&format!(", restores v{from}"));
        }
        Line::from(vec![Span::styled(format!("v{:<4}", v["number"]), Style::new().fg(if now { ACCENT } else { Color::White }).add_modifier(Modifier::BOLD)), note, dim(format!("{at}  ")), dim(fit(&tail, w.saturating_sub(33)))])
    });
    let block = panel("Latest runs", false);
    let inner = block.inner(latest);
    f.render_widget(block, latest);
    // Its unrecorded handlers first, a line each: their runs are counted, not in the list.
    let mut lines: Vec<Line> = unrecorded_lines(d).into_iter().map(|(label, said)| Line::from(vec![Span::styled(format!("{label}  "), Style::new().fg(Color::White)), dim(fit(&said, (inner.width as usize).saturating_sub(label.len() + 2)))])).collect();
    let shown = (inner.height as usize).saturating_sub(lines.len()).min(d.runs.len());
    lines.extend(d.runs.iter().rev().take(shown).map(|r| run_row(d, r, inner.width as usize)));
    if lines.is_empty() {
        f.render_widget(Paragraph::new(dim("No runs yet: c calls it.")), inner);
    } else {
        f.render_widget(Paragraph::new(lines), inner);
    }
    app.hit(latest, Click::Section(Section::Runs));
}

// ── the split panes ──────────────────────────────────────────────────────────────────────────

/// Its unrecorded handlers, by label, each with what its tally says (docs/L4s/request-path-L4.md,
/// C5): "not recorded: 1204 runs, 0.4 ms each, 2 failed, the last 503: why".
pub(crate) fn unrecorded_lines(d: &Detail) -> Vec<(String, String)> {
    let Some(tallies) = d.entry["tallies"].as_object() else { return vec![] };
    tallies
        .iter()
        .map(|(label, t)| {
            let (runs, failed, total) = (t["runs"].as_u64().unwrap_or(0), t["failed"].as_u64().unwrap_or(0), t["total_ms"].as_f64().unwrap_or(0.0));
            let mut said = format!("not recorded: {runs} run{}", if runs == 1 { "" } else { "s" });
            if runs > 0 {
                said.push_str(&format!(", {:.1} ms each", total / runs as f64));
            }
            if failed > 0 {
                said.push_str(&format!(", {failed} failed{}", last_failure(&t["last_failure"])));
            }
            (label.clone(), said)
        })
        .collect()
}

/// A tally's last failure, as a line says it: ", the last 503: why" (nothing when there is none).
fn last_failure(f: &serde_json::Value) -> String {
    let Some(status) = f["status"].as_u64() else { return String::new() };
    match f["error"].as_str() {
        Some(e) => format!(", the last {status}: {e}"),
        None => format!(", the last {status}"),
    }
}

/// How many runs of its unrecorded handlers were counted.
fn unrecorded_runs(d: &Detail) -> u64 {
    d.entry["tallies"].as_object().map_or(0, |t| t.values().filter_map(|t| t["runs"].as_u64()).sum())
}

/// A run on one line: its number, when, its version, what came in, its answer, how long.
fn run_row(d: &Detail, r: &Value, width: usize) -> Line<'static> {
    let at = at_ms(&r["at_ms"]);
    let when = if width >= 60 { format!("{:<15}", at.get(5..19).unwrap_or(&at)) } else { format!("{:<9}", at.get(11..19).unwrap_or(&at)) };
    let version = d.ran_on(r).map(|n| format!("v{n}")).unwrap_or_default();
    let answer = match (r["status"].as_u64(), r["error"].is_string()) {
        (_, true) => coloured(format!("{:>6}", "failed"), BAD),
        (Some(s), _) if s >= 400 => coloured(format!("{s:>6}"), BAD),
        (Some(s), _) => coloured(format!("{s:>6}"), GOOD),
        (None, _) => coloured(format!("{:>6}", "ok"), GOOD),
    };
    let duration = format!("{:>9}", took(r["duration_ms"].as_f64().unwrap_or(0.0)));
    let fixed = 7 + when.chars().count() + 4 + 6 + 9;
    let what = pad(&came_in(r), width.saturating_sub(fixed).max(4));
    Line::from(vec![dim(format!("#{:<6}", r["seq"].as_u64().unwrap_or_default())), dim(when), dim(format!("{version:<4}")), Span::raw(what), answer, dim(duration)])
}

/// A tag for a moment in the history: what kind, in its colour.
fn tag(kind: &str) -> Span<'static> {
    let colour = match kind {
        "deploy" => ACCENT,
        "restore" => GOOD,
        "grant" => Color::Magenta,
        "lifecycle" => WARN,
        "failed" => BAD,
        _ => DIM,
    };
    Span::styled(format!("{:<10}", fit(kind, 9)), Style::new().fg(colour).add_modifier(Modifier::BOLD))
}

fn history_row(d: &Detail, m: Moment, width: usize, first: bool, last: bool) -> Line<'static> {
    let rail = dim(if last && first { "─ " } else if first { "┌ " } else if last { "└ " } else { "├ " });
    match m {
        Moment::Run(i) => {
            let r = &d.runs[i];
            let mut line = run_row(d, r, width.saturating_sub(12));
            let kind = if failed(r) { "failed" } else { "run" };
            line.spans.insert(0, tag(kind));
            line.spans.insert(0, rail);
            line
        }
        Moment::Event(i) => {
            let e = &d.events[i];
            let at = at_ms(&e["at_ms"]);
            let by = e["by"].as_str().map(|b| format!("  by {b}")).unwrap_or_default();
            let what = e["what"].as_str().unwrap_or_default();
            Line::from(vec![rail, tag(e["kind"].as_str().unwrap_or("event")), dim(format!("{:<7}", "")), dim(format!("{:<15}", at.get(5..19).unwrap_or(&at))), bold(fit(what, width.saturating_sub(36))), dim(fit(&by, width.saturating_sub(36 + what.chars().count())))])
        }
    }
}

fn access_row(a: &Access, width: usize) -> Line<'static> {
    match a {
        Access::Grant => Line::from(bold("Its grant")),
        Access::Secrets => Line::from(bold("Its secrets")),
        Access::Item { item, how } => {
            let (mark, colour, note) = match how {
                Held::Granted => ("✓ ", GOOD, ""),
                Held::Revoked => ("✗ ", WARN, "  revoked"),
                Held::NotGranted => ("! ", BAD, "  not granted"),
            };
            Line::from(vec![Span::raw("  "), coloured(mark, colour), Span::raw(fit(&agent::describe(item), width.saturating_sub(18))), coloured(note, colour)])
        }
        Access::Secret { name, set } => Line::from(vec![Span::raw("  "), if *set { coloured("● ", GOOD) } else { coloured("○ ", WARN) }, Span::raw(name.clone()), if *set { dim("  set") } else { coloured("  not set", WARN) }]),
    }
}

/// A row of a graph: its line, its lanes (the dot green where the newest run reached it), then
/// the statement as written; a trigger in the accent, heading its band.
pub(super) fn graph_row(g: &GraphView, i: usize, width: usize) -> Line<'static> {
    let (_, _, lanes) = &g.rows[i];
    let Some(row) = g.row(i) else { return Line::default() };
    let reached = (row.line..=row.last).any(|n| g.reached.contains(&n));
    let dot = if reached { GOOD } else if row.head { ACCENT } else if row.effect { Color::White } else { Color::Gray };
    let (before, after) = lanes.split_once('●').unwrap_or((lanes, ""));
    let mut spans = vec![dim(format!("{:>4} ", row.line)), Span::styled(before.to_string(), Style::new().fg(Color::Indexed(244))), Span::styled("●", Style::new().fg(dot)), Span::styled(format!("{after}  "), Style::new().fg(Color::Indexed(244)))];
    let used = 5 + lanes.chars().count() + 2;
    let rest = width.saturating_sub(used);
    if row.head {
        let does = if row.does.is_empty() { String::new() } else { format!("  {}", row.does) };
        spans.push(Span::styled(fit(&row.text, rest.saturating_sub(does.chars().count())), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(dim(does));
    } else if g.every {
        spans.push(bold(pad(&row.name, 16)));
        spans.push(dim(pad(&row.does, 14)));
        spans.push(dim(fit(&row.text, rest.saturating_sub(30))));
    } else {
        // What it is, at the right, apart from the statement.
        let does = format!("  {}", fit(&row.does, 16));
        let style = if row.effect { Style::new().fg(Color::White) } else { Style::new() };
        spans.push(Span::styled(pad(&fit(&row.text, rest.saturating_sub(does.chars().count())), rest.saturating_sub(does.chars().count())), style));
        spans.push(dim(does));
    }
    // Over a run, what it didn't play is dimmed.
    if g.dim && !reached {
        return Line::from(spans.into_iter().map(|s| Span::styled(s.content, Style::new().fg(DARK))).collect::<Vec<_>>());
    }
    Line::from(spans)
}

/// A graph's rows, or why it has none.
pub(super) fn graph_list(f: &mut Frame, app: &App, area: Rect, g: &GraphView, sel: usize, focused: bool, of: List) {
    match &g.error {
        Some(e) => f.render_widget(Paragraph::new(vec![Line::from(coloured(" It doesn't compile, so it has no graph yet:", BAD)), Line::from(format!(" {e}"))]).wrap(ratatui::widgets::Wrap { trim: false }), area),
        None if g.rows.is_empty() => f.render_widget(Paragraph::new(dim(" Nothing in it yet.")), area),
        None => list(f, app, area, of, g.rows.len(), Some(sel), focused, |i, w| graph_row(g, i, w)),
    }
}

/// A graph's list title: how many rows of how many nodes, and what `a` does.
pub(super) fn graph_title(g: &GraphView) -> String {
    let rows = g.rows.len().saturating_sub(g.graph.bands.len());
    if g.every { format!("Every node ({})", g.graph.nodes) } else { format!("Graph: {rows} statements of {} nodes", g.graph.nodes) }
}

/// The graph's switch, a button at the right of its panel's bottom edge, whether the panel
/// has the keys or not: a shows every node, or the statements again. The panel's other keys,
/// when it has them, keep to the left of it.
pub(super) fn every_chip(f: &mut Frame, app: &App, panel: Rect, g: &GraphView) {
    let what = if g.every { " statements " } else { " every node " };
    let width = (3 + what.chars().count()) as u16;
    if panel.width < width + 12 || panel.height < 3 {
        return;
    }
    if let Some(keys) = app.keys_at.borrow_mut().as_mut().filter(|k| **k == panel) {
        keys.width -= width + 1;
    }
    let rect = Rect { x: panel.x + panel.width - width - 2, y: panel.y + panel.height - 1, width, height: 1 };
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" a ", Style::new().fg(Color::Black).bg(Color::Gray)), dim(what.to_string())])), rect);
    app.hit(rect, Click::Every);
}

fn split(f: &mut Frame, app: &mut App, area: Rect) {
    let wide = area.width >= 100;
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).areas(area) } else { Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area) };
    let Some(d) = app.detail.as_ref() else { return };
    let s = app.flow.section;
    let sel = app.flow.sel();
    let focused = !app.flow.right;
    let (title, n) = match s {
        Section::Runs => {
            let counted: u64 = unrecorded_runs(d);
            // Runs of unrecorded handlers, counted, not logged: a line each in the latest runs.
            let more = if counted > 0 { format!(", +{counted} unrecorded") } else { String::new() };
            (format!("Runs, newest first ({}{more})", d.runs.len()), d.runs.len())
        }
        Section::History => ("History, newest first: one chain".to_string(), d.moments().len()),
        Section::State => (format!("Its state ({})", app.flow.cells.len()), app.flow.cells.len()),
        Section::Graph => (app.flow.graph.as_ref().map(graph_title).unwrap_or_else(|| "Graph".into()), app.flow.graph.as_ref().map_or(0, |g| g.rows.len())),
        _ => ("Access".to_string(), access_rows(d).len()),
    };
    let block = panel(&title, focused);
    if focused {
        super::keys_here(app, left);
    }
    let inner = block.inner(left);
    f.render_widget(block, left);
    if let (Section::Graph, Some(g)) = (s, app.flow.graph.as_ref()) {
        every_chip(f, app, left, g);
        graph_list(f, app, inner, g, sel, focused, List::Section);
    } else if n == 0 {
        let empty = match s {
            Section::Runs => "No runs yet: c calls it, and each run shows here.",
            Section::State => "Nothing kept yet.",
            _ => "Nothing yet.",
        };
        f.render_widget(Paragraph::new(dim(format!(" {empty}"))), inner);
    } else {
        let app_ref: &App = app;
        match s {
            Section::Runs => list(f, app_ref, inner, List::Section, n, Some(sel), focused, |i, w| run_row(d, &d.runs[d.runs.len() - 1 - i], w)),
            Section::History => {
                let moments = d.moments();
                list(f, app_ref, inner, List::Section, n, Some(sel), focused, |i, w| history_row(d, moments[i], w, i == 0, i + 1 == n));
            }
            Section::State => {
                let cells = &app_ref.flow.cells;
                list(f, app_ref, inner, List::Section, n, Some(sel), focused, |i, w| {
                    let c = &cells[i];
                    let value = if c.collection { dim("a collection".to_string()) } else { Span::raw(fit(&agent::show(&c.value), w.saturating_sub(24))) };
                    Line::from(vec![bold(pad(&c.name, 22)), value])
                });
            }
            _ => {
                let rows = access_rows(d);
                list(f, app_ref, inner, List::Section, n, Some(sel), focused, |i, w| access_row(&rows[i], w));
            }
        }
    }
    pane(f, app, right);
}

/// The right pane: its tabs, then what the picked row holds under the tab picked.
fn pane(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.flow.right;
    if focused {
        super::keys_here(app, area);
    }
    let block = Block::new().borders(Borders::ALL).border_type(ratatui::widgets::BorderType::Rounded).border_style(Style::new().fg(if focused { ACCENT } else { DIM }));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let names = tabs(app.flow.section);
    // The tabs sit on the border's top line.
    let mut x = area.x + 2;
    for (i, name) in names.iter().enumerate() {
        let here = i == app.flow.tab();
        let label = format!(" {name} ");
        let w = label.chars().count() as u16;
        if x + w >= area.x + area.width {
            break;
        }
        let style = if here { Style::new().fg(Color::Black).bg(if focused { ACCENT } else { Color::Gray }).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
        let rect = Rect { x, y: area.y, width: w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        app.hit(rect, Click::DetailTab(i));
        x += w + 1;
    }
    let width = inner.width.saturating_sub(1) as usize;
    let lines: Vec<Line> = match &app.flow.pane {
        Some((_, rows)) => render_rows(rows, width),
        None => vec![Line::from(dim("Pick a row on the left."))],
    };
    let max = lines.len().saturating_sub(inner.height as usize);
    app.flow.scroll = app.flow.scroll.min(max);
    let top = app.flow.scroll;
    app.hit(inner, Click::Row(List::Detail, 0));
    f.render_widget(Paragraph::new(lines.clone()).scroll((top as u16, 0)), inner);
    scrollbar(f, inner, lines.len(), top);
}

/// A step on a timeline: its label, then a bar for when it ran, from 0 to `total`, then how long.
pub(super) fn bar(label: &str, from: f64, to: f64, total: f64, note: &str, width: usize) -> Line<'static> {
    let label_w = (width / 3).clamp(10, 28);
    let note_w = 9;
    let bar_w = width.saturating_sub(label_w + note_w + 1).max(4);
    let a = ((from / total) * bar_w as f64).floor() as usize;
    let b = (((to / total) * bar_w as f64).ceil() as usize).max(a + 1).min(bar_w);
    let a = a.min(b.saturating_sub(1));
    Line::from(vec![
        Span::raw(pad(label, label_w)),
        Span::styled("·".repeat(a), Style::new().fg(Color::Indexed(238))),
        Span::styled("█".repeat(b - a), Style::new().fg(ACCENT)),
        Span::styled("·".repeat(bar_w - b), Style::new().fg(Color::Indexed(238))),
        dim(format!(" {note:>8}")),
    ])
}

/// Long text cut into lines of `width`.
fn wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = vec![];
    for line in s.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
        }
        for chunk in chars.chunks(width) {
            out.push(chunk.iter().collect());
        }
    }
    out
}

pub(super) fn render_rows(rows: &[Row], width: usize) -> Vec<Line<'static>> {
    let key_width = rows.iter().filter_map(|r| if let Row::Field(k, ..) = r { Some(k.chars().count()) } else { None }).max().unwrap_or(0).min(16) + 2;
    let mut lines = vec![];
    for (i, r) in rows.iter().enumerate() {
        match r {
            Row::Text(Tone::Head, s) => {
                if i > 0 {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled(s.clone(), Style::new().add_modifier(Modifier::BOLD))));
            }
            Row::Text(t, s) => lines.extend(wrap(s, width).into_iter().map(|l| Line::from(Span::styled(l, tone(*t))))),
            Row::Field(k, v, t) => {
                let parts = wrap(v, width.saturating_sub(key_width));
                for (j, p) in parts.into_iter().enumerate() {
                    let key = if j == 0 { pad(k, key_width) } else { " ".repeat(key_width) };
                    lines.push(Line::from(vec![dim(key), Span::styled(p, tone(*t))]));
                }
            }
            Row::Bar { label, from, to, total, note } => lines.push(bar(label, *from, *to, *total, note, width)),
            Row::Code { n, text, notes } => {
                let ran = !notes.is_empty();
                lines.push(Line::from(vec![dim(format!("{n:>3} ")), Span::styled(fit(text, width.saturating_sub(4)), if ran { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() })]));
                let indent = text.len() - text.trim_start().len() + 4;
                for note in notes {
                    for l in wrap(note, width.saturating_sub(indent)) {
                        lines.push(Line::from(vec![Span::raw(" ".repeat(indent)), Span::styled(l, Style::new().fg(Color::Indexed(180)))]));
                    }
                }
            }
        }
    }
    lines
}
