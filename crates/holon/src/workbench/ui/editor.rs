//! The editor: its tabs, the text (coloured, its mistakes underlined, the selection and what find
//! found marked), completions under the cursor, find, the problems, what it may do, and a run
//! laid over its text.

use holon_author::agent;
use holon_author::editing::{self, Class};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::{bold, coloured, dim, fit, list, pad, panel, ACCENT, BAD, DARK, DIM, GOOD, PICKED, WARN};
use crate::workbench::app::{self, App, Click, List, RunPane, Show, Tab};

fn colour(class: Class) -> Style {
    let c = match class {
        Class::Comment => return Style::new().fg(DIM).add_modifier(Modifier::ITALIC),
        Class::Keyword => Color::Magenta,
        Class::Noun => Color::Yellow,
        Class::Function => Color::LightBlue,
        Class::Definition => Color::Cyan,
        Class::Parameter => Color::LightRed,
        Class::Member => Color::White,
        Class::Property => Color::LightCyan,
        Class::Method => Color::LightGreen,
        Class::Path => Color::Green,
        Class::String => Color::Green,
        Class::Escape => Color::LightGreen,
        Class::Interpolation => Color::Yellow,
        Class::Number | Class::Boolean => Color::LightMagenta,
        Class::Operator => Color::Gray,
    };
    Style::new().fg(c)
}

const SELECTED: Color = Color::Indexed(24);
const CURRENT_LINE: Color = Color::Indexed(234);

/// One line of a tab's text, coloured, its mistakes underlined and the selection marked, from
/// character `left`.
fn text_line(tab: &Tab, row: usize, left: usize, width: usize) -> Vec<Span<'static>> {
    let line = &tab.buf.lines[row];
    let mut styles: Vec<Style> = vec![Style::new(); line.chars().count()];
    // The colours are as last checked: while typing, a line's may lag a key behind.
    if let Some(colours) = tab.check.colours.get(row) {
        for &(from, to, class) in colours {
            let to = to.min(styles.len());
            for s in styles.iter_mut().take(to).skip(from) {
                *s = colour(class);
            }
        }
    }
    for m in &tab.check.mistakes {
        let (start, end) = (m.span.start, m.span.end);
        if start.line as usize != row + 1 {
            continue;
        }
        let from = start.col.saturating_sub(1) as usize;
        let to = if end.line == start.line && end.col > start.col { end.col.saturating_sub(1) as usize } else if end.line > start.line { styles.len() } else { from + 1 };
        for s in styles.iter_mut().take(to).skip(from) {
            *s = s.fg(BAD).add_modifier(Modifier::UNDERLINED);
        }
    }
    let mut selected = 0..0;
    if let Some(((r0, c0), (r1, c1))) = tab.buf.selection()
        && (r0..=r1).contains(&row)
    {
        let from = if row == r0 { c0 } else { 0 };
        let to = if row == r1 { c1 } else { styles.len() + 1 };
        selected = from..to;
    }
    for (i, s) in styles.iter_mut().enumerate() {
        if selected.contains(&i) {
            *s = s.bg(SELECTED);
        }
    }
    let mut spans: Vec<Span<'static>> = vec![];
    for (c, style) in line.chars().zip(styles).skip(left).take(width) {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    // A selection that runs past a line's end shows there too.
    if selected.end > line.chars().count() && left <= line.chars().count() {
        spans.push(Span::styled(" ", Style::new().bg(SELECTED)));
    }
    spans
}

pub(super) fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.tabs.is_empty() {
        let lines = vec![
            Line::default(),
            Line::from(bold("No flow open.")),
            Line::default(),
            Line::from("Start a draft (n, or ctrl+t anywhere), or open one: a sample from the library (6), a flow on the server from the flows page (3), or a file with :open <file>."),
            Line::from(dim("ctrl+p finds any of them by name.")),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("Editor", true)), area);
        return;
    }
    app.sync_graph();
    let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
    draw_tabs(f, app, bar);
    if app.tabs[app.tab].runs.is_some() {
        return draw_runs_view(f, app, body);
    }
    match app.show {
        Show::Graph => draw_graph_view(f, app, body),
        Show::Both => {
            let [text, graph] = halves(body, 55);
            draw_text(f, app, text);
            draw_graph_beside(f, app, graph, "at the cursor");
        }
        Show::Code => {
            let side_width = if body.width >= 100 { 42 } else if body.width >= 70 { 32 } else { 0 };
            let [main, side] = Layout::horizontal([Constraint::Min(20), Constraint::Length(side_width)]).areas(body);
            draw_text(f, app, main);
            if side_width > 0 {
                draw_side(f, app, side);
            }
        }
    }
}

/// Two panes side by side, the first `left` percent; one over the other when narrow.
fn halves(area: Rect, left: u16) -> [Rect; 2] {
    if area.width >= 90 {
        Layout::horizontal([Constraint::Percentage(left), Constraint::Percentage(100 - left)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area)
    }
}

/// The graph beside the text, or over a run: its rows, the picked one marked.
fn draw_graph_beside(f: &mut Frame, app: &App, area: Rect, what: &str) {
    let Some(g) = app.tabs[app.tab].graph.as_ref() else { return };
    let statements = g.view.rows.len().saturating_sub(g.view.graph.bands.len());
    let block = panel(&format!("Graph: {statements} statements · {what}"), false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    super::flow::graph_list(f, app, inner, &g.view, g.sel, true, List::Graph);
}

/// The graph alone: its statements with their wires on the left, the picked one on the right
/// (over a run, with what the run gave there).
fn draw_graph_view(f: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] = halves(area, 56);
    let Some(g) = app.tabs[app.tab].graph.as_ref() else { return };
    let run = app.tabs[app.tab].runs.as_ref().and_then(|r| r.view.as_ref()).and_then(|v| v.as_ref().ok());
    let focused = !g.right || run.is_some();
    let what = if run.is_some() { "what the run played" } else { "as typed" };
    let block = panel(&format!("{}  ({what})", super::flow::graph_title(&g.view)), focused);
    let inner = block.inner(left);
    f.render_widget(block, left);
    super::flow::graph_list(f, app, inner, &g.view, g.sel, focused, List::Graph);
    let block = panel(if run.is_some() { "In this run" } else { "Statement" }, g.right);
    let inner = block.inner(right);
    f.render_widget(block, right);
    let rows = match run {
        Some(v) => g.view.run_pane(g.sel, v),
        None => g.view.pane(g.sel, 0, None),
    };
    let lines = super::flow::render_rows(&rows, inner.width.saturating_sub(1) as usize);
    let top = g.scroll.min(lines.len().saturating_sub(inner.height as usize));
    app.hit(inner, Click::Row(List::GraphPane, 0));
    f.render_widget(Paragraph::new(lines.clone()).scroll((top as u16, 0)), inner);
    super::scrollbar(f, inner, lines.len(), top);
    if let Some(g) = app.tabs[app.tab].graph.as_mut() {
        g.scroll = top;
    }
}

/// The tab bar, each tab a button with its own ✕; on the right, Code / Both / Graph, and Runs.
fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut x = area.x;
    let right = 30;
    for (i, t) in app.tabs.iter().enumerate() {
        let here = i == app.tab;
        let style = if here { Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray).bg(Color::Indexed(236)) };
        let name = format!(" {}{} ", fit(&t.name, 24), if t.dirty() { " ●" } else { "" });
        let w = name.chars().count() as u16;
        if x + w + 3 > area.x + area.width.saturating_sub(right + 4) {
            f.render_widget(Paragraph::new(dim(format!(" +{}", app.tabs.len() - i))), Rect { x, width: 4, ..area });
            x += 5;
            break;
        }
        let rect = Rect { x, width: w, ..area };
        f.render_widget(Paragraph::new(Span::styled(name, style)), rect);
        app.hit(rect, Click::Tab(i));
        let close = Rect { x: x + w, width: 2, ..area };
        f.render_widget(Paragraph::new(Span::styled("✕ ", if here { style } else { Style::new().fg(DIM).bg(Color::Indexed(236)) })), close);
        app.hit(close, Click::CloseTab(i));
        x += w + 3;
    }
    // A new tab, with a draft in it (ctrl+t), as a browser's +.
    let plus = Rect { x, width: 3, ..area };
    f.render_widget(Paragraph::new(Span::styled(" + ", Style::new().fg(ACCENT).bg(Color::Indexed(236)).add_modifier(Modifier::BOLD))), plus);
    app.hit(plus, Click::Key(KeyCode::Char('t'), KeyModifiers::CONTROL));
    // What it shows, each a button (ctrl+b goes round them), then the runs (ctrl+r) over it.
    let runs = app.tabs[app.tab].runs.is_some();
    let modes = [
        (" Code ", app.show == Show::Code, GOOD, Click::Show(Show::Code)),
        (" Both ", app.show == Show::Both, GOOD, Click::Show(Show::Both)),
        (" Graph ", app.show == Show::Graph, GOOD, Click::Show(Show::Graph)),
        (" Runs ", runs, WARN, Click::Key(KeyCode::Char('r'), KeyModifiers::CONTROL)),
    ];
    let width: u16 = modes.iter().map(|(l, ..)| l.chars().count() as u16 + 1).sum();
    let mut x = area.x + area.width.saturating_sub(width);
    for (label, on, colour, click) in modes {
        let w = label.chars().count() as u16;
        if x + w > area.x + area.width {
            break;
        }
        let r = Rect { x, width: w, ..area };
        f.render_widget(Paragraph::new(Span::styled(label, if on { Style::new().fg(Color::Black).bg(colour) } else { Style::new().fg(Color::Gray).bg(Color::Indexed(236)) })), r);
        app.hit(r, click);
        x += w + 1;
    }
}

fn draw_text(f: &mut Frame, app: &mut App, area: Rect) {
    let problems = app.tabs[app.tab].check.mistakes.len();
    let problems_height = if problems == 0 { 3 } else { (problems.min(4) + 2) as u16 };
    let [text, below] = Layout::vertical([Constraint::Min(3), Constraint::Length(problems_height)]).areas(area);
    let typing = app.typing && app.modal.is_none() && app.prompt.is_none();
    let find = app.find.is_some();
    let height = text.height.saturating_sub(2 + find as u16) as usize;
    app.text_height = height;
    let tab = &mut app.tabs[app.tab];
    tab.buf.scroll(height);
    let gutter = tab.buf.lines.len().to_string().len().max(2);
    let width = text.width.saturating_sub(gutter as u16 + 3) as usize;
    let (row, col) = (tab.buf.row, tab.buf.col.min(tab.buf.lines[tab.buf.row].chars().count()));
    let left = (col + 1).saturating_sub(width);
    let bad_lines: Vec<usize> = tab.check.mistakes.iter().map(|m| m.span.start.line as usize).collect();
    let mut lines = vec![];
    for r in tab.buf.top..(tab.buf.top + height).min(tab.buf.lines.len()) {
        let n = r + 1;
        let gutter_style = if bad_lines.contains(&n) { Style::new().fg(BAD) } else if r == row { Style::new().fg(Color::Gray) } else { Style::new().fg(DIM) };
        let mark = if bad_lines.contains(&n) { "●" } else { " " };
        let mut spans = vec![Span::styled(format!("{n:>gutter$}{mark}"), gutter_style)];
        spans.extend(text_line(tab, r, left, width));
        let mut line = Line::from(spans);
        if r == row && typing && tab.buf.selection().is_none() {
            line = line.style(Style::new().bg(CURRENT_LINE));
        }
        lines.push(line);
    }
    let where_ = tab.path.as_ref().map(|p| p.display().to_string()).or(tab.slug.as_ref().map(|s| format!("{s} on the server"))).unwrap_or_else(|| "not saved".into());
    let title = format!("{}  {}:{}", fit(&where_, 48), row + 1, col + 1);
    let block = panel(&title, typing);
    let inner = block.inner(text);
    f.render_widget(Paragraph::new(lines).block(block), text);
    let content = Rect { x: inner.x + gutter as u16 + 1, y: inner.y, width: inner.width.saturating_sub(gutter as u16 + 1), height: height as u16 };
    let (top, cursor) = (tab.buf.top, (content.x + (col - left) as u16, content.y + (row - tab.buf.top) as u16));
    app.hit(content, Click::Text { x: content.x, y: content.y, top, left });
    if let Some(q) = &app.find {
        let bar = Rect { y: inner.y + inner.height - 1, height: 1, ..inner };
        let line = Line::from(vec![Span::styled(" Find ", Style::new().fg(Color::Black).bg(WARN)), Span::raw(format!(" {q}")), Span::styled(" ", Style::new().bg(Color::Gray)), dim("   enter next · shift+enter previous · esc close")]);
        f.render_widget(Paragraph::new(line).style(Style::new().bg(Color::Indexed(236))), bar);
        f.set_cursor_position((bar.x + 7 + q.chars().count() as u16, bar.y));
    } else if typing {
        f.set_cursor_position(cursor);
    }
    if typing && app.complete.is_some() {
        completions(f, app, cursor, text);
    }

    let tab = &app.tabs[app.tab];
    let block = panel(&format!("Problems ({problems})"), false);
    let inner = block.inner(below);
    f.render_widget(block, below);
    if problems == 0 {
        f.render_widget(Paragraph::new(coloured(if tab.edited.is_some() { "…" } else { "✓ no mistakes" }, GOOD)), inner);
    } else {
        let mistakes = &tab.check.mistakes;
        list(f, app, inner, List::Problems, mistakes.len(), None, false, |i, w| {
            let m = &mistakes[i];
            let at = format!("{}:{}  ", m.span.start.line, m.span.start.col);
            Line::from(vec![coloured(at.clone(), BAD), Span::raw(fit(m.message.lines().next().unwrap_or(""), w.saturating_sub(at.len())))])
        });
    }
}

/// The completions offered, in a box under the cursor (over it when there's no room below),
/// with what the picked one is beside it.
fn completions(f: &mut Frame, app: &App, (x, y): (u16, u16), within: Rect) {
    let Some(c) = &app.complete else { return };
    let n = c.items.len().min(8);
    let width = (c.items.iter().map(|i| i.label.chars().count() + kind(i.kind).len() + 4).max().unwrap_or(10) as u16).clamp(16, 44);
    let height = n as u16 + 2;
    let below = y + 1 + height <= within.y + within.height;
    let top = if below { y + 1 } else { y.saturating_sub(height) };
    let left = x.saturating_sub(c.back as u16 + 1).min((within.x + within.width).saturating_sub(width));
    let rect = Rect { x: left, y: top, width, height };
    f.render_widget(Clear, rect);
    let block = panel("", true).title("");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let items = &c.items;
    list(f, app, inner, List::Complete, items.len(), Some(c.sel), true, |i, w| {
        let it = &items[i];
        let kind = kind(it.kind);
        Line::from(vec![Span::raw(pad(&it.label, w.saturating_sub(kind.len() + 1))), dim(kind)])
    });
    // What it is, when there's room on the right.
    let Some(item) = items.get(c.sel) else { return };
    let doc: Vec<String> = [Some(item.detail.clone()), item.doc.clone()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    let room = (within.x + within.width).saturating_sub(rect.x + rect.width);
    if doc.is_empty() || room < 24 {
        return;
    }
    let w = room.min(50);
    let text = doc.join("\n\n").replace("**", "").replace('`', "");
    let lines = text.lines().count().min(10) as u16 + 2;
    let side = Rect { x: rect.x + rect.width, y: rect.y, width: w, height: lines.max(3).min(within.height) };
    f.render_widget(Clear, side);
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(panel("", false)), side);
}

/// What a completion is, in a word.
fn kind(k: editing::Kind) -> &'static str {
    match k {
        editing::Kind::Keyword => "keyword",
        editing::Kind::Function => "leaf",
        editing::Kind::Variable => "name",
        editing::Kind::Module => "noun",
        editing::Kind::Member => "member",
    }
}

/// Markdown as plain lines: fences and emphasis marks left out.
fn plain(markdown: &str) -> Vec<Line<'static>> {
    markdown.lines().filter(|l| !l.trim_start().starts_with("```")).map(|l| Line::from(l.replace("**", "").replace('`', ""))).collect()
}

fn draw_side(f: &mut Frame, app: &App, area: Rect) {
    let tab = &app.tabs[app.tab];
    let mut may: Vec<Line> = vec![];
    match &tab.check.plan {
        Ok(plan) => {
            if plan.may.is_empty() {
                may.push(Line::from(dim("only compute")));
            }
            for i in &plan.may {
                let new = tab.deployed.as_ref().is_some_and(|d| !d.contains(i));
                may.push(Line::from(vec![if new { coloured("+ ", WARN) } else { dim("· ") }, Span::raw(agent::describe(i))]));
            }
            if tab.deployed.as_ref().is_some_and(|d| plan.may.iter().any(|i| !d.contains(i))) {
                may.push(Line::from(coloured("+ new since deployed: deploying asks you", WARN)));
            }
            let reach = if !plan.routes.is_empty() { plan.routes.join("  ") } else { plan.interval_seconds.map(|i| format!("every {i} s")).unwrap_or_else(|| "started by hand".into()) };
            may.push(Line::default());
            may.push(Line::from(dim(reach)));
        }
        Err(_) => may.push(Line::from(dim("(once it compiles)"))),
    }
    let may_height = (may.len() as u16 + 2).min(area.height / 2);
    let [top, bottom] = Layout::vertical([Constraint::Length(may_height), Constraint::Min(3)]).areas(area);
    let title = match tab.target() {
        Some(s) => format!("It may ({s})"),
        None => "It may".into(),
    };
    f.render_widget(Paragraph::new(may).wrap(Wrap { trim: false }).block(panel(&title, false)), top);
    let hover = editing::hover(&tab.buf.text(), app::cursor(tab));
    let lines = match hover {
        Some(h) => plain(&h.text),
        None => vec![Line::from(dim("Put the cursor on a name to read about it."))],
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("Inspector", false)), bottom);
}

/// A run over the text it ran on: the code (what it played lit, the rest dimmed, the step picked
/// marked, each statement's value under it), its graph, or both. Under it, as in the web editor,
/// a panel: the runs on its left; on its right, the run picked: its details, why it failed, and
/// its timeline, one step a row, the picked one marked in the code and the graph too.
fn draw_runs_view(f: &mut Frame, app: &mut App, body: Rect) {
    let Some(runs) = app.tabs[app.tab].runs.as_ref() else { return };
    let height = if body.height >= 24 { (body.height * 2 / 5).min(18) } else { body.height / 2 };
    let [top, bottom] = Layout::vertical([Constraint::Min(3), Constraint::Length(height)]).areas(body);
    match &runs.view {
        Some(Ok(_)) => match app.show {
            Show::Code => draw_run_code(f, app, top),
            Show::Both => {
                let [code, graph] = halves(top, 55);
                draw_run_code(f, app, code);
                draw_graph_beside(f, app, graph, "what the run played");
            }
            Show::Graph => draw_graph_view(f, app, top),
        },
        Some(Err(e)) => f.render_widget(Paragraph::new(coloured(e.clone(), BAD)).wrap(Wrap { trim: false }).block(panel("Run", true)), top),
        None => f.render_widget(Paragraph::new(dim("No runs yet: call it, and they show here.")).block(panel("Runs", true)), top),
    }
    let left_width = (bottom.width * 2 / 5).clamp(24.min(bottom.width), 64);
    let [left, right] = Layout::horizontal([Constraint::Length(left_width), Constraint::Min(10)]).areas(bottom);
    draw_run_list(f, app, left);
    draw_run_pane(f, app, right);
}

/// The deployment's runs, newest first: number, time, what started it, its answer, how long.
fn draw_run_list(f: &mut Frame, app: &App, area: Rect) {
    let Some(runs) = app.tabs[app.tab].runs.as_ref() else { return };
    let block = panel(&format!("Runs of {} ({})  ·  ↑ ↓", runs.slug, runs.entries.len()), true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if runs.entries.is_empty() {
        return f.render_widget(Paragraph::new(dim(" no runs yet: call it")), inner);
    }
    list(f, app, inner, List::Runs, runs.entries.len(), Some(runs.sel), true, |i, w| {
        let e = &runs.entries[i];
        let s = |k: &str| e.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
        let seq = format!("#{:<4}", e.get("seq").and_then(|v| v.as_u64()).unwrap_or(0));
        let at = e.get("at_ms").and_then(|v| v.as_u64()).map(|ms| agent::utc(ms / 1000).chars().skip(11).take(8).collect::<String>()).unwrap_or_default();
        let what = match s("kind") {
            Some("request") => format!("{} {}", s("method").unwrap_or("?"), s("url").unwrap_or("?")),
            kind => s("route").or(kind).unwrap_or("?").to_string(),
        };
        let failed = e.get("error").is_some_and(|x| !x.is_null());
        let status = e.get("status").and_then(|v| v.as_u64());
        let answer = if failed { "err".to_string() } else { status.map(|s| s.to_string()).unwrap_or_default() };
        let ms = e.get("duration_ms").and_then(|v| v.as_f64()).map(|ms| format!("{ms:.1}")).unwrap_or_default();
        let tail = format!(" {answer:>3} {ms:>7}");
        let room = w.saturating_sub(seq.len() + at.len() + 2 + tail.chars().count());
        let bad = failed || status.is_some_and(|s| s >= 400);
        Line::from(vec![
            dim(seq),
            dim(format!("{at} ")),
            Span::raw(pad(&fit(&what, room), room)),
            coloured(format!(" {answer:>3}"), if bad { BAD } else { GOOD }),
            dim(format!(" {ms:>7}")),
        ])
    });
}

/// The run picked, on the panel's right: its tabs (Details, Errors when it failed, Timeline),
/// as the web editor's, and the one picked.
fn draw_run_pane(f: &mut Frame, app: &App, area: Rect) {
    let Some(runs) = app.tabs[app.tab].runs.as_ref() else { return };
    let block = panel("", true).title("");
    let inner = block.inner(area);
    f.render_widget(block, area);
    // The tabs, on the panel's top edge, each a button; tab goes round them.
    let mut x = area.x + 1;
    for p in runs.panes() {
        let label = match p {
            RunPane::Details => " Details ".to_string(),
            RunPane::Errors => format!(" Errors ({}) ", runs.errors().len()),
            RunPane::Timeline => " Timeline ".to_string(),
        };
        let w = label.chars().count() as u16;
        if x + w > area.x + area.width {
            break;
        }
        let on = p == runs.pane;
        let colour = if p == RunPane::Errors { BAD } else { ACCENT };
        let style = if on { Style::new().fg(Color::Black).bg(colour).add_modifier(Modifier::BOLD) } else { Style::new().fg(if p == RunPane::Errors { BAD } else { Color::Gray }) };
        let r = Rect { x, y: area.y, width: w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(label, style)), r);
        app.hit(r, Click::RunPane(p));
        x += w + 1;
    }
    let steps = runs.steps();
    let note = match runs.pane {
        RunPane::Timeline if !steps.is_empty() => format!(" step {} of {} · ← → ", runs.step + 1, steps.len()),
        _ => " tab: next ".to_string(),
    };
    let w = note.chars().count() as u16;
    if x + w + 1 < area.x + area.width {
        f.render_widget(Paragraph::new(dim(note)), Rect { x: area.x + area.width - w - 1, y: area.y, width: w, height: 1 });
    }
    let width = inner.width.saturating_sub(1) as usize;
    match runs.pane {
        RunPane::Timeline => draw_timeline(f, app, inner),
        RunPane::Errors => {
            let text = |line: u32| match &runs.view {
                Some(Ok(v)) => v.text.get(line as usize - 1).map(|l| l.trim().to_string()).unwrap_or_default(),
                _ => String::new(),
            };
            let mut lines = vec![];
            for (line, why) in runs.errors() {
                match line {
                    Some(n) => {
                        lines.push(Line::from(vec![coloured(format!("line {n}  "), WARN), Span::raw(fit(&text(n), width.saturating_sub(10)))]));
                        lines.push(Line::from(vec![Span::raw("  "), coloured(why, BAD)]));
                    }
                    None => lines.push(Line::from(vec![coloured("failed: ", BAD), Span::raw(why)])),
                }
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        RunPane::Details => {
            let mut lines: Vec<Line> = vec![];
            if let Some(e) = runs.entries.get(runs.sel) {
                lines.push(Line::from(Span::styled(agent::run_line(e), Style::new().fg(Color::White))));
            }
            match &runs.view {
                Some(Ok(v)) => {
                    if v.no_steps {
                        lines.push(Line::from(dim("This run recorded no steps: the values are left out.")));
                    }
                    for l in &v.after {
                        let (name, rest) = l.split_once(": ").unwrap_or((l.as_str(), ""));
                        let colour = if name.starts_with("failed") { BAD } else { ACCENT };
                        lines.push(Line::from(vec![coloured(format!("{}  ", name.trim_end()), colour), Span::raw(rest.to_string())]));
                    }
                    if v.after.is_empty() {
                        lines.push(Line::from(dim("It answered nothing and did nothing outside: its values are in the code above.")));
                    }
                }
                Some(Err(e)) => lines.push(Line::from(coloured(e.clone(), BAD))),
                None => lines.push(Line::from(dim("No runs yet: call it, and they show here."))),
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

/// The text a run ran on: what it played lit, the rest dimmed, the step picked marked, and
/// each statement's value under it.
fn draw_run_code(f: &mut Frame, app: &App, area: Rect) {
    let tab = &app.tabs[app.tab];
    let Some(runs) = &tab.runs else { return };
    let Some(Ok(v)) = &runs.view else { return };
    let steps = runs.steps();
    let at = runs.at();
    let lit = tab.graph.as_ref().map(|g| g.view.lit()).unwrap_or_else(|| steps.iter().copied().collect());
    let width = v.text.len().to_string().len();
    let mut lines: Vec<Line> = vec![];
    let mut at_row = 0;
    for (i, l) in v.text.iter().enumerate() {
        let n = (i + 1) as u32;
        let here = at == Some(n);
        if here {
            at_row = lines.len();
        }
        let (number, text) = match (here, lit.contains(&n)) {
            (true, _) => (Style::new().fg(WARN), Style::new().fg(Color::White).add_modifier(Modifier::BOLD)),
            (_, true) => (Style::new().fg(Color::Gray), Style::new().fg(Color::White)),
            _ => (Style::new().fg(DARK), Style::new().fg(DARK)),
        };
        let mark = if here { "▶" } else { " " };
        let line = Line::from(vec![Span::styled(format!("{n:>width$}{mark}"), number), Span::styled(l.clone(), text)]);
        lines.push(if here { line.style(Style::new().bg(PICKED)) } else { line });
        let indent = l.len() - l.trim_start().len();
        for note in v.notes.get(&n).into_iter().flatten() {
            let colour = if here { WARN } else if lit.contains(&n) { Color::Indexed(180) } else { DARK };
            lines.push(Line::from(vec![Span::raw(format!("{:>width$} {}", "", " ".repeat(indent))), coloured(note.clone(), colour)]));
        }
    }
    let height = area.height.saturating_sub(2) as usize;
    let scroll = at_row.saturating_sub(height / 3) as u16;
    let step = if steps.is_empty() { "no steps recorded".to_string() } else { format!("step {} of {}", runs.step + 1, steps.len()) };
    let head = fit(&v.head, (area.width as usize).saturating_sub(step.len() + 12));
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(panel(&format!("{step}  ·  {head}"), true)), area);
}

/// A run's timeline: each statement as it ran, a bar for when, the step picked marked.
fn draw_timeline(f: &mut Frame, app: &App, inner: Rect) {
    let Some(runs) = app.tabs[app.tab].runs.as_ref() else { return };
    let Some(Ok(v)) = &runs.view else { return f.render_widget(Paragraph::new(dim(" No timeline: this run can't be shown over the text.")), inner) };
    let total = v.steps.iter().map(|s| s.end_us).fold(0.0, f64::max).max(1.0);
    let steps = runs.steps();
    if steps.is_empty() {
        return f.render_widget(Paragraph::new(dim(" It ran nothing this text says.")), inner);
    }
    let head = if v.steps.is_empty() { " This run recorded no timing: its steps, in order.".to_string() } else { format!(" {} steps over {}", v.steps.len(), crate::workbench::flow::took(total / 1000.0)) };
    let [head_area, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    f.render_widget(Paragraph::new(dim(head)), head_area);
    let name = |line: u32| format!("{line:>3} {}", v.text.get(line as usize - 1).map(|l| l.trim()).unwrap_or_default());
    list(f, app, rows, List::Timeline, steps.len(), Some(runs.step), true, |i, w| match v.steps.get(i) {
        Some(s) => super::flow::bar(&name(s.line), s.start_us, s.end_us, total, &crate::workbench::flow::took((s.end_us - s.start_us) / 1000.0), w),
        None => Line::from(Span::raw(fit(&name(steps[i]), w))),
    });
}
