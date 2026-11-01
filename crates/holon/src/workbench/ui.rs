//! Drawing `holon editor`: a bar along the top, the pages down the side, the page, and two lines
//! at the bottom (what just happened, and the keys that work here), with a dialog over them.

use holon_author::editing::{self, Class};
use holon_author::{agent, samples};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;

use super::app::{self, App, FlowTab, Modal, Page, Tab, PAGES};
use super::client::{self, Client};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const GOOD: Color = Color::Green;

/// Below this width the sidebar folds away.
const SIDEBAR_FROM: u16 = 110;
const SIDEBAR: u16 = 21;

fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(DIM))
}

fn bold(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().add_modifier(Modifier::BOLD))
}

fn coloured(s: impl Into<String>, c: Color) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(c))
}

fn panel(title: &str, focused: bool) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
        .title(Span::styled(format!(" {title} "), Style::new().add_modifier(Modifier::BOLD)))
}

/// `s` cut to `n` characters, with an ellipsis when cut.
fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A row of a list, marked when selected.
fn row(selected: bool, spans: Vec<Span<'static>>) -> Line<'static> {
    let mut line = Line::from([vec![Span::raw(if selected { "▸ " } else { "  " })], spans].concat());
    if selected {
        line = line.style(Style::new().bg(Color::Indexed(236)).add_modifier(Modifier::BOLD));
    }
    line
}

/// The rows from `top` that keep `sel` among `height` shown.
fn window(sel: usize, height: usize) -> usize {
    (sel + 1).saturating_sub(height.max(1))
}

pub(crate) fn draw(f: &mut Frame, app: &mut App) {
    let [top, body, note, keys] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());
    draw_top(f, app, top);
    let main = if body.width >= SIDEBAR_FROM {
        let [side, main] = Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(10)]).areas(body);
        draw_sidebar(f, app, side);
        main
    } else {
        body
    };
    match app.page {
        Page::Overview => draw_overview(f, app, main),
        Page::Approvals => draw_approvals(f, app, main),
        Page::Flows => draw_flows(f, app, main),
        Page::Flow => draw_flow(f, app, main),
        Page::Editor => draw_editor(f, app, main),
        Page::Library => draw_library(f, app, main),
    }
    draw_note(f, app, note);
    draw_keys(f, app, keys);
    if let Some(modal) = &app.modal {
        draw_modal(f, app, modal, f.area());
    }
}

fn draw_top(f: &mut Frame, app: &App, area: Rect) {
    let mut left = vec![Span::styled(" holon ", Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)), Span::raw(" ")];
    match (&app.client, &app.offline) {
        (Some(c), None) => {
            left.push(coloured("● ", GOOD));
            left.push(Span::raw(c.server()));
            if let Some(me) = &c.me {
                left.push(dim(format!("  as {me}")));
            }
        }
        (Some(c), Some(_)) => {
            left.push(coloured("● ", BAD));
            left.push(Span::raw(c.server()));
            left.push(coloured("  can't reach it", BAD));
        }
        (None, _) => left.push(coloured("○ no server: editing files only", WARN)),
    }
    let mut right = vec![];
    if let Some(c) = &app.client {
        right.push(Span::raw(format!("{} deployed", c.deployments.len())));
        let waiting = app.waiting().len();
        right.push(dim(" · "));
        right.push(if waiting > 0 { coloured(format!("{waiting} waiting for you"), WARN) } else { dim("nothing waiting") });
        right.push(dim(" · "));
    }
    right.push(Span::raw(format!("{} tab{} ", app.tabs.len(), if app.tabs.len() == 1 { "" } else { "s" })));
    let right_width: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width as u16)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Line::from(right)), r);
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::default()];
    let waiting = app.waiting().len();
    for (i, (page, name)) in PAGES.iter().enumerate() {
        let here = app.page == *page;
        let label = match page {
            Page::Flow => match &app.detail {
                Some(d) => fit(&d.slug, 13),
                None => continue,
            },
            Page::Approvals if waiting > 0 => format!("{name} {waiting}"),
            Page::Editor if !app.tabs.is_empty() => format!("{name} {}", app.tabs.len()),
            _ => name.to_string(),
        };
        let style = if here { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new() };
        let indent = if *page == Page::Flow { "  └ " } else { " " };
        lines.push(Line::from(vec![dim(format!(" {} ", i + 1)), Span::styled(format!("{indent}{label}"), style)]));
    }
    lines.push(Line::default());
    lines.push(Line::from(dim(" : command  ? keys")));
    f.render_widget(Paragraph::new(lines).block(Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(DIM))), area);
}

fn draw_note(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(p) = &app.prompt {
        Line::from(vec![coloured(":", ACCENT), Span::raw(p.clone()), Span::styled(" ", Style::new().bg(Color::Gray))])
    } else if let Some(n) = &app.note {
        Line::from(coloured(format!(" {}", n.text), if n.error { BAD } else { GOOD }))
    } else if let Some(why) = &app.offline {
        Line::from(coloured(format!(" {}", why.lines().next().unwrap_or("")), WARN))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_keys(f: &mut Frame, app: &App, area: Rect) {
    let keys: &[(&str, &str)] = if app.modal.is_some() {
        match app.modal {
            Some(Modal::Approve { .. }) => &[("enter", "approve and deploy"), ("esc", "cancel")],
            Some(Modal::Confirm { .. }) => &[("enter", "yes"), ("esc", "no")],
            _ => &[("↑↓", "scroll"), ("esc", "close")],
        }
    } else if app.prompt.is_some() {
        &[("enter", "run"), ("esc", "cancel"), (":help", "the commands")]
    } else {
        match app.page {
            Page::Editor if app.tabs.is_empty() => &[("6", "library"), (":open", "a file"), ("3", "flows"), ("q", "quit")],
            Page::Editor if app.tabs[app.tab].runs.is_some() => &[("↑↓", "run"), ("n/N", "next/previous step"), ("v", "back to editing"), ("^d", "deploy")],
            Page::Editor if app.typing => &[("esc", "keys for pages"), ("^s", "save"), ("^d", "deploy"), ("^r", "runs"), ("^f", "format"), ("^←→", "tabs"), ("^w", "close")],
            Page::Editor => &[("i", "type"), ("[ ]", "tabs"), ("s", "save"), ("d", "deploy"), ("v", "runs"), ("w", "close"), ("1-6", "pages"), ("q", "quit")],
            Page::Flow => &[("←→", "status/runs/history"), ("enter", "explain a run"), ("e", "edit"), ("g/p/x", "run/pause/stop"), ("esc", "flows")],
            Page::Overview => &[("↑↓", "pick"), ("enter", "open"), ("e", "edit"), ("r", "refresh"), ("1-6", "pages"), ("q", "quit")],
            Page::Approvals => &[("↑↓", "pick"), ("enter", "review and approve"), ("1-6", "pages"), ("q", "quit")],
            Page::Flows => &[("↑↓", "pick"), ("enter", "open"), ("e", "edit"), ("1-6", "pages"), ("q", "quit")],
            Page::Library => &[("↑↓", "pick"), ("enter", "open a copy"), ("1-6", "pages"), ("q", "quit")],
        }
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, what) in keys {
        spans.push(Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(Color::Gray)));
        spans.push(dim(format!(" {what}  ")));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── pages ─────────────────────────────────────────────────────────────────────────────────

fn offline(f: &mut Frame, app: &App, area: Rect, title: &str) -> bool {
    if app.client.is_some() {
        return false;
    }
    let why = app.offline.clone().unwrap_or_default();
    let lines = vec![
        Line::default(),
        Line::from(coloured("No server reached.", WARN)),
        Line::default(),
        Line::from(why),
        Line::default(),
        Line::from(dim("Start one with holon serve, then press r. Files still open, check and format: :open <file>, or the library (6).")),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(title, true)), area);
    true
}

/// A deployment's state, coloured.
fn status(d: &Value) -> Span<'static> {
    let s = d["status"].as_str().unwrap_or("?");
    if d.get("attention").is_some_and(|a| !a.is_null()) || d.get("listen_error").is_some_and(|a| !a.is_null()) {
        return coloured("needs you", WARN);
    }
    match s {
        "running" => coloured("running", GOOD),
        "paused" => coloured("paused", WARN),
        s => coloured(s.to_string(), BAD),
    }
}

/// How a deployment is reached: its port, or its clock.
fn reach(d: &Value) -> String {
    if let Some(p) = d["port"].as_u64() {
        format!(":{p}")
    } else if let Some(i) = d["interval_seconds"].as_u64() {
        format!("every {i}s")
    } else {
        "by hand".into()
    }
}

fn deployment_row(d: &Value, selected: bool, width: usize) -> Line<'static> {
    let slug = d["slug"].as_str().unwrap_or("?");
    let unit = d["unit"].as_str().unwrap_or("");
    let runs = d["trigger_count"].as_u64().unwrap_or(0);
    let failed = d["error_count"].as_u64().unwrap_or(0);
    let mut spans = vec![bold(format!("{:<22}", fit(slug, 21))), status(d)];
    let pad = 11usize.saturating_sub(spans[1].content.chars().count());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(dim(format!("{:<10}", &unit[..unit.len().min(8)])));
    spans.push(Span::raw(format!("{:<10}", reach(d))));
    spans.push(Span::raw(format!("{runs:>6} run{}", if runs == 1 { " " } else { "s" })));
    spans.push(if failed > 0 { coloured(format!("{failed:>5} failed"), BAD) } else { dim(format!("{:>12}", "")) });
    if width > 90 {
        let by = d["deployed_by"].as_str().map(|b| format!("by {b}, ")).unwrap_or_default();
        spans.push(dim(format!("   {by}{}", crate::mcp::when(&d["deployed_at_epoch_seconds"]))));
    }
    row(selected, spans)
}

fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Overview") {
        return;
    }
    let c = app.client.as_ref().unwrap();
    let waiting = app.waiting();
    let wait_height = (waiting.len().clamp(1, 8) + 2) as u16;
    let [deps, waits, saved] = Layout::vertical([Constraint::Min(5), Constraint::Length(wait_height), Constraint::Length(4)]).areas(area);
    let sel = app.sel[0];
    let height = deps.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = vec![];
    if c.deployments.is_empty() {
        lines.push(Line::from(dim("Nothing deployed yet. Open a sample (6), or a file with :open <file>, and deploy it with ctrl+d.")));
    }
    let skip = window(sel, height);
    for (i, d) in c.deployments.iter().enumerate().skip(skip).take(height) {
        lines.push(deployment_row(d, i == sel, deps.width as usize));
    }
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Deployed on {}", c.server()), true)), deps);

    let lines: Vec<Line> = if waiting.is_empty() {
        vec![Line::from(dim("Nothing waits for a person."))]
    } else {
        waiting.iter().take(8).map(|(slug, w)| Line::from(vec![coloured("● ", WARN), bold(format!("{slug}  ")), Span::raw(w.clone())])).collect()
    };
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Waiting for you ({})", waiting.len()), false)), waits);

    let names: Vec<String> = c.flows.iter().filter_map(|f| f["name"].as_str().map(str::to_string)).collect();
    let line = if names.is_empty() { Line::from(dim("No saved flows.")) } else { Line::from(Span::raw(names.join(", "))) };
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }).block(panel(&format!("Saved flows ({})", names.len()), false)), saved);
}

fn draw_approvals(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Approvals") {
        return;
    }
    let waiting = app.waiting();
    let sel = app.sel[1];
    let mut lines = vec![Line::from(dim("What waits for a person: you approve a flow by deploying it, after reading what it may do.")), Line::default()];
    if waiting.is_empty() {
        lines.push(Line::from(coloured("Nothing waits for you.", GOOD)));
    }
    let height = area.height.saturating_sub(4) as usize;
    for (i, (slug, w)) in waiting.iter().enumerate().skip(window(sel, height)).take(height) {
        lines.push(row(i == sel, vec![bold(format!("{:<22}", fit(slug, 21))), Span::raw(w.clone())]));
    }
    f.render_widget(Paragraph::new(lines).block(panel("Approvals", true)), area);
}

fn draw_flows(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Flows") {
        return;
    }
    let c = app.client.as_ref().unwrap();
    let names = app.flow_names();
    let sel = app.sel[2];
    let mut lines = vec![Line::from(vec![dim(format!("  {:<22}{:<11}{:<10}{:<10}", "name", "state", "version", "reach")), dim("saved")])];
    let height = area.height.saturating_sub(3) as usize;
    for (i, name) in names.iter().enumerate().skip(window(sel, height)).take(height) {
        let saved = c.saved(name).map(|s| {
            let by = s["saved_by"].as_str().map(|b| format!(" by {b}")).unwrap_or_default();
            format!("saved{by}, {}", crate::mcp::when(&s["saved_at_epoch_seconds"]))
        });
        let mut spans = vec![bold(format!("{:<22}", fit(name, 21)))];
        match c.deployment(name) {
            Some(d) => {
                let st = status(d);
                let pad = 11usize.saturating_sub(st.content.chars().count());
                let unit = d["unit"].as_str().unwrap_or("");
                spans.extend([st, Span::raw(" ".repeat(pad)), dim(format!("{:<10}", &unit[..unit.len().min(8)])), Span::raw(format!("{:<10}", reach(d)))]);
            }
            None => spans.push(coloured(format!("{:<31}", "not deployed"), DIM)),
        }
        spans.push(dim(saved.unwrap_or_default()));
        lines.push(row(i == sel, spans));
    }
    if names.is_empty() {
        lines.push(Line::from(dim("No flows on this server yet.")));
    }
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Flows ({})", names.len()), true)), area);
}

fn draw_flow(f: &mut Frame, app: &App, area: Rect) {
    let Some(d) = &app.detail else { return };
    let [head, tabs, body] = Layout::vertical([Constraint::Length(2), Constraint::Length(1), Constraint::Min(3)]).areas(area);
    let e = &d.entry;
    let unit = e["unit"].as_str().unwrap_or("");
    let runs = e["trigger_count"].as_u64().unwrap_or(0);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::styled(format!(" {} ", d.slug), Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)), Span::raw("  "), status(e), dim(format!("   version {}   {}   {runs} runs, {} failed", &unit[..unit.len().min(12)], reach(e), e["error_count"].as_u64().unwrap_or(0)))]),
            Line::from(dim(format!(" deployed{} at {}", e["deployed_by"].as_str().map(|b| format!(" by {b}")).unwrap_or_default(), crate::mcp::when(&e["deployed_at_epoch_seconds"])))),
        ]),
        head,
    );
    let mut spans = vec![Span::raw(" ")];
    for (t, name) in [(FlowTab::Status, "Status"), (FlowTab::Runs, "Runs"), (FlowTab::History, "History")] {
        let style = if app.flow_tab == t { Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(DIM) };
        spans.push(Span::styled(format!(" {name} "), style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);
    match app.flow_tab {
        FlowTab::Status => {
            let lines = flow_status(app, d);
            let scroll = app.flow_scroll.min(lines.len().saturating_sub(1)) as u16;
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)).block(panel("Status", true)), body);
        }
        FlowTab::Runs => {
            let height = body.height.saturating_sub(2) as usize;
            let sel = app.flow_sel;
            let mut lines: Vec<Line> = vec![];
            if d.runs.is_empty() {
                lines.push(Line::from(dim("No runs yet.")));
            }
            for (i, r) in d.runs.iter().rev().enumerate().skip(window(sel, height)).take(height) {
                let failed = r.get("error").is_some_and(|e| !e.is_null());
                let text = fit(&agent::run_line(r), body.width.saturating_sub(6) as usize);
                lines.push(row(i == sel, vec![if failed { coloured(text, BAD) } else { Span::raw(text) }]));
            }
            f.render_widget(Paragraph::new(lines).block(panel(&format!("Runs, newest first ({})", d.runs.len()), true)), body);
        }
        FlowTab::History => {
            let lines: Vec<Line> = if d.timeline.is_empty() { vec![Line::from(dim("Nothing yet."))] } else { d.timeline.iter().rev().map(|l| Line::from(l.clone())).collect() };
            let scroll = app.flow_scroll.min(lines.len().saturating_sub(1)) as u16;
            f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(panel("History, newest first", true)), body);
        }
    }
}

fn flow_status(app: &App, d: &client::Detail) -> Vec<Line<'static>> {
    let mut lines = vec![];
    let items = client::strings(&d.manifest, "items");
    let not_granted = client::strings(&d.manifest, "not_granted");
    if let Ok(plan) = d.text.as_deref().map_err(String::clone).and_then(agent::plan)
        && !plan.routes.is_empty()
    {
        lines.push(Line::from(bold("Routes")));
        lines.extend(plan.routes.iter().map(|r| Line::from(format!("  {r}"))));
        lines.push(Line::default());
    }
    lines.push(Line::from(bold("It may")));
    if items.is_empty() {
        lines.push(Line::from(dim("  only compute")));
    }
    for i in &items {
        let missing = not_granted.contains(i);
        lines.push(Line::from(vec![if missing { coloured("  ✗ ", BAD) } else { coloured("  ✓ ", GOOD) }, Span::raw(agent::describe(i)), if missing { coloured("  (its grant doesn't cover this)", BAD) } else { Span::raw("") }]));
    }
    let reads: Vec<String> = client::strings(&d.manifest, "capabilities").iter().filter_map(|c| c.strip_prefix("SecretRead(\"")?.strip_suffix("\")").map(str::to_string)).collect();
    if !reads.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(bold("Secrets it reads")));
        for r in reads {
            let set = d.secrets.contains(&r);
            lines.push(Line::from(vec![Span::raw(format!("  {r}  ")), if set { coloured("set", GOOD) } else { coloured("not set: an admin sets it in the browser's editor", WARN) }]));
        }
    }
    let waits = crate::mcp::waits_for(&d.entry, app.client.as_ref().and_then(|c: &Client| c.saved(&d.slug)));
    lines.push(Line::default());
    lines.push(Line::from(bold("Waiting for a person")));
    if waits.is_empty() {
        lines.push(Line::from(dim("  nothing")));
    }
    lines.extend(waits.into_iter().map(|w| Line::from(vec![coloured("  ● ", WARN), Span::raw(w)])));
    lines.push(Line::default());
    lines.push(Line::from(bold("Its text, as deployed (e edits it)")));
    match &d.text {
        Ok(t) => lines.extend(t.lines().map(|l| Line::from(format!("  {l}")))),
        Err(e) => lines.push(Line::from(coloured(format!("  can't be shown: {e}"), BAD))),
    }
    lines
}

fn draw_library(f: &mut Frame, app: &App, area: Rect) {
    let all = samples::samples();
    let sel = app.sel[5].min(all.len().saturating_sub(1));
    let [list, about] = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
    let height = list.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = all.iter().enumerate().skip(window(sel, height)).take(height).map(|(i, s)| row(i == sel, vec![bold(format!("{:<20}", fit(s.name, 19))), dim(s.title.to_string())])).collect();
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Samples ({})", all.len()), true)), list);
    if let Some(s) = all.get(sel) {
        let mut lines = vec![Line::from(bold(s.title.to_string())), Line::default(), Line::from(s.description.to_string()), Line::default()];
        lines.push(Line::from(vec![dim("Shows  "), Span::raw(s.shows.join(", "))]));
        lines.push(Line::default());
        lines.push(Line::from(dim("Try it")));
        lines.extend(s.try_it.lines().map(|l| Line::from(format!("  {l}"))));
        lines.push(Line::default());
        lines.push(Line::from(dim("Enter opens a copy in a tab: it is yours to edit, save and deploy.")));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(s.name, false)), about);
    }
}

// ── the editor ────────────────────────────────────────────────────────────────────────────

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

/// One line of a tab's text, coloured, with its mistakes underlined, from character `left`.
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
    let mut spans: Vec<Span<'static>> = vec![];
    for (c, style) in line.chars().zip(styles).skip(left).take(width) {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    spans
}

fn draw_editor(f: &mut Frame, app: &mut App, area: Rect) {
    if app.tabs.is_empty() {
        let lines = vec![
            Line::default(),
            Line::from(bold("No flow open.")),
            Line::default(),
            Line::from("Open one: a sample from the library (6), a flow on the server from the flows page (3), or a file with :open <file>."),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("Editor", true)), area);
        return;
    }
    let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
    // The tab bar, with the mode on the right.
    let mut spans = vec![];
    for (i, t) in app.tabs.iter().enumerate() {
        let here = i == app.tab;
        let style = if here { Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
        let mark = if t.dirty() { " ●" } else { "" };
        spans.push(Span::styled(format!(" {}{mark} ", fit(&t.name, 24)), style));
        spans.push(Span::raw(" "));
    }
    let viewing = app.tabs[app.tab].runs.is_some();
    let mode = vec![
        Span::styled(" Edit ", if viewing { Style::new().fg(DIM) } else { Style::new().fg(Color::Black).bg(GOOD) }),
        Span::raw(" "),
        Span::styled(" View runs ", if viewing { Style::new().fg(Color::Black).bg(WARN) } else { Style::new().fg(DIM) }),
    ];
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(19)]).areas(bar);
    f.render_widget(Paragraph::new(Line::from(spans)), l);
    f.render_widget(Paragraph::new(Line::from(mode)), r);

    let side_width = if body.width >= 100 { 42 } else if body.width >= 70 { 32 } else { 0 };
    let [main, side] = Layout::horizontal([Constraint::Min(20), Constraint::Length(side_width)]).areas(body);
    if viewing {
        draw_runs_view(f, app, main, side);
    } else {
        draw_text(f, app, main);
        if side_width > 0 {
            draw_side(f, app, side);
        }
    }
}

fn draw_text(f: &mut Frame, app: &mut App, area: Rect) {
    let problems = app.tabs[app.tab].check.mistakes.len();
    let problems_height = if problems == 0 { 3 } else { (problems.min(4) + 2) as u16 };
    let [text, below] = Layout::vertical([Constraint::Min(3), Constraint::Length(problems_height)]).areas(area);
    let typing = app.typing && app.modal.is_none() && app.prompt.is_none();
    let height = text.height.saturating_sub(2) as usize;
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
        let mut spans = vec![Span::styled(format!("{n:>gutter$} "), gutter_style)];
        spans.extend(text_line(tab, r, left, width));
        lines.push(Line::from(spans));
    }
    let where_ = tab.path.as_ref().map(|p| p.display().to_string()).or(tab.slug.as_ref().map(|s| format!("{s} on the server"))).unwrap_or_else(|| "not saved".into());
    let title = format!("{}  {}:{}", fit(&where_, 48), row + 1, col + 1);
    f.render_widget(Paragraph::new(lines).block(panel(&title, typing)), text);
    if typing {
        let x = text.x + 1 + gutter as u16 + 1 + (col - left) as u16;
        let y = text.y + 1 + (row - tab.buf.top) as u16;
        f.set_cursor_position((x, y));
    }
    let lines: Vec<Line> = if problems == 0 {
        vec![Line::from(coloured(if tab.edited.is_some() { "…" } else { "✓ no mistakes" }, GOOD))]
    } else {
        tab.check.mistakes.iter().take(4).map(|m| Line::from(vec![coloured(format!("{}:{}  ", m.span.start.line, m.span.start.col), BAD), Span::raw(m.message.lines().next().unwrap_or("").to_string())])).collect()
    };
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Problems ({problems})"), false)), below);
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

fn draw_runs_view(f: &mut Frame, app: &App, main: Rect, side: Rect) {
    let tab = &app.tabs[app.tab];
    let Some(runs) = &tab.runs else { return };
    let side = if side.width == 0 { None } else { Some(side) };
    let [text, below] = Layout::vertical([Constraint::Min(3), Constraint::Length(6)]).areas(main);
    let noted = runs.noted();
    let at = noted.get(runs.step).copied();
    match &runs.view {
        Some(Ok(v)) => {
            let width = v.text.len().to_string().len();
            let mut lines: Vec<Line> = vec![];
            let mut at_row = 0;
            for (i, l) in v.text.iter().enumerate() {
                let n = (i + 1) as u32;
                let here = at == Some(n);
                if here {
                    at_row = lines.len();
                }
                let style = if here { Style::new().bg(Color::Indexed(236)).add_modifier(Modifier::BOLD) } else { Style::new() };
                lines.push(Line::from(vec![dim(format!("{n:>width$} ")), Span::raw(l.clone())]).style(style));
                let indent = l.len() - l.trim_start().len();
                for note in v.notes.get(&n).into_iter().flatten() {
                    lines.push(Line::from(vec![Span::raw(format!("{:>width$} {}", "", " ".repeat(indent))), coloured(note.clone(), if here { WARN } else { Color::Indexed(180) })]));
                }
            }
            let height = text.height.saturating_sub(2) as usize;
            let scroll = at_row.saturating_sub(height / 3) as u16;
            let step = if noted.is_empty() { "no steps recorded".to_string() } else { format!("step {} of {}", runs.step + 1, noted.len()) };
            let head = fit(&v.head, (text.width as usize).saturating_sub(step.len() + 12));
            f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(panel(&format!("{step}  ·  {head}"), true)), text);
            let mut after: Vec<Line> = v.after.iter().map(|l| Line::from(l.clone())).collect();
            if v.no_steps {
                after.insert(0, Line::from(dim("This run recorded no steps: the values are left out.")));
            }
            f.render_widget(Paragraph::new(after).wrap(Wrap { trim: false }).block(panel("What it did", false)), below);
        }
        Some(Err(e)) => f.render_widget(Paragraph::new(coloured(e.clone(), BAD)).block(panel("Run", true)), main),
        None => f.render_widget(Paragraph::new(dim("No runs yet: call it, and they show here.")).block(panel("Runs", true)), main),
    }
    if let Some(side) = side {
        let height = side.height.saturating_sub(2) as usize;
        let lines: Vec<Line> = runs
            .entries
            .iter()
            .enumerate()
            .skip(window(runs.sel, height))
            .take(height)
            .map(|(i, e)| {
                let failed = e.get("error").is_some_and(|x| !x.is_null());
                let line = agent::run_line(e);
                let short = fit(line.split(", at ").next().unwrap_or(&line), side.width.saturating_sub(5) as usize);
                row(i == runs.sel, vec![if failed { coloured(short, BAD) } else { Span::raw(short) }])
            })
            .collect();
        f.render_widget(Paragraph::new(lines).block(panel(&format!("Runs of {}", runs.slug), false)), side);
    }
}

// ── dialogs ───────────────────────────────────────────────────────────────────────────────

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

const HELP: &[(&str, &str)] = &[
    ("Pages", ""),
    ("1 … 6", "overview, approvals, flows, the open flow, the editor, the library"),
    ("↑ ↓  enter", "pick a row, and open it"),
    ("r", "read the server again (it is read every few seconds too)"),
    (":", "a command: open <file|slug>, new <sample>, w [file], deploy, flow <slug>,"),
    ("", "run|pause|stop <slug>, server <port|url>, q, q!"),
    ("q", "quit"),
    ("", ""),
    ("Editor", ""),
    ("esc / i", "leave the text for the page keys, and come back to it"),
    ("ctrl+s", "save: to its file, else on the server, not deployed"),
    ("ctrl+d", "deploy: shows what it may do and what you approve, then deploys"),
    ("ctrl+r", "view runs: what each line did in a run; n and N step through"),
    ("ctrl+f", "format, as holon fmt writes it"),
    ("ctrl+← →", "the tab before or after; ctrl+w closes this one"),
    ("", ""),
    ("A flow", ""),
    ("← →", "its status, runs and history"),
    ("enter", "on a run: its text with what each statement did"),
    ("e", "open its text in the editor"),
    ("g  p  x", "run, pause or stop it"),
];

fn draw_modal(f: &mut Frame, app: &App, modal: &Modal, area: Rect) {
    let (title, lines, width, scroll): (String, Vec<Line>, u16, usize) = match modal {
        Modal::Approve { slug, plan, staged, .. } => {
            let mut lines = vec![Line::from(dim(format!("Deploying \"{slug}\" ({}) to {}.", &plan.unit[..12], app.client.as_ref().map(Client::server).unwrap_or_default()))), Line::default()];
            lines.push(Line::from(bold("It may")));
            if plan.may.is_empty() {
                lines.push(Line::from(dim("  only compute")));
            }
            for i in &plan.may {
                let new = staged.needs.contains(i);
                lines.push(Line::from(vec![if new { coloured("  + ", WARN) } else { dim("  · ") }, Span::raw(agent::describe(i))]));
            }
            for n in staged.needs.iter().filter(|n| !plan.may.contains(n)) {
                lines.push(Line::from(vec![coloured("  + ", WARN), Span::raw(agent::describe(n))]));
            }
            lines.push(Line::default());
            if staged.needs.is_empty() {
                lines.push(Line::from(coloured("Nothing new to approve: its grant covers all of this.", GOOD)));
            } else {
                lines.push(Line::from(coloured(format!("You approve the {} marked +, as you, by deploying it.", staged.needs.len()), WARN)));
            }
            if !staged.missing_secrets.is_empty() {
                lines.push(Line::from(coloured(format!("Not set yet: {} (a run that reads one gets nothing until an admin sets it).", staged.missing_secrets.join(", ")), WARN)));
            }
            if !plan.routes.is_empty() {
                lines.push(Line::default());
                lines.extend(plan.routes.iter().map(|r| Line::from(dim(format!("  {r}")))));
            }
            (if staged.needs.is_empty() { format!("Deploy {slug}?") } else { format!("Approve and deploy {slug}?") }, lines, 84, 0)
        }
        Modal::Explain { title, lines, scroll } => (fit(title, 70), lines.iter().map(|l| Line::from(l.clone())).collect(), 100, *scroll),
        Modal::Confirm { title, lines, .. } => (title.clone(), lines.iter().map(|l| Line::from(l.clone())).collect(), 64, 0),
        Modal::Help { scroll } => {
            let lines = HELP.iter().map(|(k, what)| if what.is_empty() && !k.is_empty() { Line::from(bold(k.to_string())) } else { Line::from(vec![coloured(format!("  {k:<12}"), ACCENT), Span::raw(what.to_string())]) }).collect();
            ("Keys".into(), lines, 90, *scroll)
        }
    };
    let height = (lines.len() as u16 + 2).max(5);
    let rect = centred(area, width, height);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll as u16, 0)).block(panel(&title, true)), rect);
}
