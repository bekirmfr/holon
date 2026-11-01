//! The admin page: its parts as tabs, then a list on the left and the picked row on the right.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;

use super::flow::render_rows;
use super::{bold, coloured, dim, fit, list, pad, panel, scrollbar, ACCENT, BAD, DIM, WARN};
use crate::workbench::admin::{Part, PARTS, SERVER_ROWS};
use crate::workbench::app::{App, Click, List};

pub(super) fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if super::offline(f, app, area, "Admin") {
        return;
    }
    let [bar, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).areas(area);
    draw_parts(f, app, bar);
    if let Some(e) = &app.admin.error {
        let lines = vec![Line::from(coloured(" This page is the server's admins':", BAD)), Line::from(format!(" {e}")), Line::from(dim(" Sign in as an admin (holon login), or ask one."))];
        return f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    }
    let wide = body.width >= 100;
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).areas(body) } else { Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body) };
    draw_list(f, app, left);
    draw_pane(f, app, right);
}

fn counts(app: &App) -> [Option<usize>; 6] {
    let a = &app.admin;
    let deployed = app.client.as_ref().map_or(0, |c| c.deployments.len());
    [Some(a.users.len()), Some(a.tokens.len()), Some(a.folders.len()), Some(a.databases.len()), Some(deployed), None]
}

fn draw_parts(f: &mut Frame, app: &App, area: Rect) {
    let mut x = area.x + 1;
    for ((p, name), n) in PARTS.iter().zip(counts(app)) {
        let here = app.admin.part == *p;
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
        app.hit(Rect { height: 2, ..rect }, Click::Part(*p));
        x += w + 1;
    }
    let rule = Rect { x, y: area.y + 1, width: (area.x + area.width).saturating_sub(x), height: 1 };
    f.render_widget(Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::new().fg(DIM))), rule);
    f.render_widget(Paragraph::new(Span::styled("─", Style::new().fg(DIM))), Rect { x: area.x, y: area.y + 1, width: 1, height: 1 });
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("-")
}

/// A binding's row: its name, its access, where it is, and for whom.
fn binding(v: &Value, kind: &str, width: usize) -> Line<'static> {
    let place = v["path"].as_str().or(v["connection"].as_str()).or(v["address"].as_str()).unwrap_or("");
    let only = v["deployment"].as_str().map(|d| format!("  {d} only")).unwrap_or_default();
    Line::from(vec![bold(pad(s(&v["name"]), 16)), Span::raw(pad(kind, 11)), dim(fit(&format!("{place}{only}"), width.saturating_sub(28)))])
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let a = &app.admin;
    let name = PARTS.iter().find(|(p, _)| *p == a.part).map_or("", |(_, n)| n);
    let title = match a.part {
        Part::People => format!("{name}: who signs in ({})", a.users.len()),
        Part::Keys => format!("{name}: for scripts and agents ({})", a.tokens.len()),
        Part::Folders => format!("{name} flows read and write ({})", a.folders.len()),
        Part::Databases => format!("{name} flows query ({})", a.databases.len()),
        Part::Secrets => format!("{name}: names only, never values"),
        Part::Server => "The server".to_string(),
    };
    let focused = !a.right;
    let block = panel(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let n = app.admin_rows();
    if n == 0 {
        let empty = match a.part {
            Part::People => "Nobody yet: n invites someone.",
            Part::Keys => "No API keys: n makes one.",
            Part::Folders => "No folders bound: n binds one.",
            _ => "No databases bound: n binds one.",
        };
        return f.render_widget(Paragraph::new(dim(format!(" {empty}"))), inner);
    }
    let slugs: Vec<String> = app.client.as_ref().map(|c| c.deployments.iter().filter_map(|d| d["slug"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    list(f, app, inner, List::Admin, n, Some(a.sel()), focused, |i, w| match a.part {
        Part::People => {
            let u = &a.users[i];
            let state = if u["disabled"] == true { coloured("disabled", WARN) } else if u["invited"] == true { dim("invited") } else { Span::raw("") };
            Line::from(vec![bold(pad(s(&u["name"]), 18)), Span::raw(pad(s(&u["role"]), 8)), state, dim(fit(&format!("  {}", u["email"].as_str().unwrap_or("")), w.saturating_sub(36)))])
        }
        Part::Keys => {
            let t = &a.tokens[i];
            Line::from(vec![bold(pad(s(&t["label"]), 24)), Span::raw(pad(s(&t["role"]), 8)), dim(fit(s(&t["id"]), w.saturating_sub(32)))])
        }
        Part::Folders => binding(&a.folders[i], s(&a.folders[i]["access"]), w),
        Part::Databases => binding(&a.databases[i], s(&a.databases[i]["engine"]), w),
        Part::Secrets if i == 0 => Line::from(dim("the server's environment")),
        Part::Secrets => Line::from(Span::raw(slugs.get(i - 1).cloned().unwrap_or_default())),
        Part::Server => Line::from(Span::raw(SERVER_ROWS[i])),
    });
}

fn draw_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let block = panel("", app.admin.right);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width.saturating_sub(1) as usize;
    let lines: Vec<Line> = match &app.admin.pane {
        Some((_, rows)) => render_rows(rows, width),
        None => vec![Line::from(dim("Pick a row on the left."))],
    };
    let max = lines.len().saturating_sub(inner.height as usize);
    app.admin.scroll = app.admin.scroll.min(max);
    let top = app.admin.scroll;
    app.hit(inner, Click::Row(List::AdminPane, 0));
    f.render_widget(Paragraph::new(lines.clone()).scroll((top as u16, 0)), inner);
    scrollbar(f, inner, lines.len(), top);
}
