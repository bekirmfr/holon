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
    let wide = body.width >= 100;
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).areas(body) } else { Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body) };
    draw_list(f, app, left);
    draw_pane(f, app, right);
}

fn counts(app: &App) -> [Option<usize>; PARTS.len()] {
    let a = &app.admin;
    let deployed = app.client.as_ref().map_or(0, |c| c.deployments.len());
    // A part not shown to the caller counts nothing.
    PARTS.map(|(p, _)| if a.refused[p as usize].is_some() { None } else { match p {
        Part::People => Some(a.users.len()),
        Part::Groups => Some(a.groups.len()),
        Part::Roles => Some(a.roles.len()),
        Part::Bindings => Some(a.bindings.len()),
        Part::History => Some(a.versions.len()),
        Part::Yours => Some(a.owned.len()),
        Part::Keys => Some(a.tokens.len()),
        Part::Folders => Some(a.folders.len()),
        Part::Databases => Some(a.databases.len()),
        Part::Secrets => Some(deployed),
        Part::Server => None,
    } })
}

/// The parts' tabs, from the first that lets the current one show: a narrow bar scrolls, with
/// `‹` and `›` where parts are left out.
fn draw_parts(f: &mut Frame, app: &App, area: Rect) {
    let labels: Vec<String> = PARTS.iter().zip(counts(app)).map(|((_, name), n)| match n {
        Some(n) => format!(" {name} {n} "),
        None => format!(" {name} "),
    }).collect();
    let widths: Vec<u16> = labels.iter().map(|l| l.chars().count() as u16 + 1).collect();
    let here = PARTS.iter().position(|(p, _)| *p == app.admin.part).unwrap_or(0);
    let room = area.width.saturating_sub(3);
    let mut first = 0;
    while first < here && widths[first..=here].iter().sum::<u16>() > room {
        first += 1;
    }
    let mut x = area.x + 1;
    if first > 0 {
        f.render_widget(Paragraph::new(dim("‹")), Rect { x: area.x, y: area.y, width: 1, height: 1 });
    }
    let mut last = first;
    for (i, ((p, _), label)) in PARTS.iter().zip(labels).enumerate().skip(first) {
        let w = label.chars().count() as u16;
        if x + w > area.x + area.width.saturating_sub(1) {
            break;
        }
        last = i;
        let current = app.admin.part == *p;
        let style = if current { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
        let rect = Rect { x, y: area.y, width: w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        f.render_widget(Paragraph::new(Span::styled(if current { "━".repeat(w as usize) } else { String::new() }, Style::new().fg(ACCENT))), Rect { y: area.y + 1, ..rect });
        app.hit(Rect { height: 2, ..rect }, Click::Part(*p));
        x += w + 1;
    }
    if last + 1 < PARTS.len() {
        f.render_widget(Paragraph::new(dim("›")), Rect { x: (area.x + area.width).saturating_sub(1), y: area.y, width: 1, height: 1 });
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
        Part::People => format!("{name}: people, agents and services ({})", a.users.len()),
        Part::Groups => format!("{name}: a role bound to one holds for each member ({})", a.groups.len()),
        Part::Roles => format!("{name}: what each allows ({})", a.roles.len()),
        Part::Bindings => format!("{name}: who holds which role, where ({})", a.bindings.len()),
        Part::History => format!("{name}: who approves what, then each version ({})", a.versions.len()),
        Part::Yours => format!("{name} and services, owned by you ({})", a.owned.len()),
        Part::Keys => format!("{name}: the agents' and services' ({})", a.tokens.len()),
        Part::Folders => format!("{name} flows read and write ({})", a.folders.len()),
        Part::Databases => format!("{name} flows query ({})", a.databases.len()),
        Part::Secrets => format!("{name}: names only, never values"),
        Part::Server => "The server".to_string(),
    };
    let focused = !a.right;
    let block = panel(&title, focused);
    if focused {
        super::keys_here(app, area);
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some(why) = &a.refused[a.part as usize] {
        let lines = vec![Line::from(coloured(" It can't be shown here:", BAD)), Line::from(format!(" {why}")), Line::from(dim(" Ask an admin for a role that holds it, or sign in as one (holon login)."))];
        return f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
    let n = app.admin_rows();
    if n == 0 {
        let empty = match a.part {
            Part::People => "Nobody yet: n invites someone, a adds an agent or a service.",
            Part::Keys => "No API keys: n makes one for an agent or a service.",
            Part::Folders => "No folders bound: n binds one.",
            Part::Groups => "No groups: n makes one.",
            Part::Roles => "No roles: n defines one.",
            Part::Bindings => "Nothing bound: n binds a role.",
            Part::Yours => "No agent or service of yours: an admin adds one, owned by you.",
            _ => "No databases bound: n binds one.",
        };
        return f.render_widget(Paragraph::new(dim(format!(" {empty}"))), inner);
    }
    let slugs: Vec<String> = app.client.as_ref().map(|c| c.deployments.iter().filter_map(|d| d["slug"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    list(f, app, inner, List::Admin, n, Some(a.sel()), focused, |i, w| match a.part {
        Part::People => {
            let u = &a.users[i];
            let state = if u["disabled"] == true { coloured("disabled", WARN) } else if u["invited"] == true { dim("invited") } else { Span::raw("") };
            // A person's email; an agent's or a service's owner.
            let after = match u["owner"].as_str() {
                Some(owner) => format!("  owned by {owner}"),
                None => format!("  {}", u["email"].as_str().unwrap_or("")),
            };
            Line::from(vec![bold(pad(s(&u["name"]), 18)), Span::raw(pad(s(&u["kind"]), 8)), Span::raw(pad(s(&u["role"]), 8)), state, dim(fit(&after, w.saturating_sub(44)))])
        }
        Part::Keys => {
            let t = &a.tokens[i];
            Line::from(vec![bold(pad(s(&t["label"]), 24)), Span::raw(pad(s(&t["principal"]), 16)), dim(fit(s(&t["id"]), w.saturating_sub(40)))])
        }
        Part::Folders => binding(&a.folders[i], s(&a.folders[i]["access"]), w),
        Part::Databases => binding(&a.databases[i], s(&a.databases[i]["engine"]), w),
        Part::Secrets if i == 0 => Line::from(dim("the server's environment")),
        Part::Secrets => Line::from(Span::raw(slugs.get(i - 1).cloned().unwrap_or_default())),
        Part::Server => Line::from(Span::raw(SERVER_ROWS[i])),
        Part::Groups => {
            let g = &a.groups[i];
            let members = g["members"].as_array().map_or(0, Vec::len);
            let bound = g["bound"].as_u64().unwrap_or(0);
            Line::from(vec![bold(pad(s(&g["name"]), 18)), Span::raw(pad(&format!("{members} member{}", if members == 1 { "" } else { "s" }), 12)), dim(fit(&format!("bound {bound}×  {}", g["about"].as_str().unwrap_or("")), w.saturating_sub(30)))])
        }
        Part::Roles => {
            let r = &a.roles[i];
            let allows = r["allows"].as_array().map_or(0, Vec::len);
            let kind = if r["builtin"] == true { "built in" } else { "defined" };
            Line::from(vec![bold(pad(s(&r["name"]), 20)), Span::raw(pad(kind, 10)), dim(fit(&format!("{allows} actions, bound {}×", r["bound"].as_u64().unwrap_or(0)), w.saturating_sub(30)))])
        }
        Part::Bindings => {
            let b = &a.bindings[i];
            let id = format!("#{}", b["id"]);
            let expired = b["expired"] == true;
            let role = if expired { dim(pad(s(&b["role"]), 16)) } else { bold(pad(s(&b["role"]), 16)) };
            let to = crate::accesscmd::subject(&b["who"]);
            let place = crate::accesscmd::place(&b["scope"]);
            Line::from(vec![dim(pad(&id, 5)), role, Span::raw(pad(&to, 20)), dim(fit(&format!("{place}{}", if expired { " (expired)" } else { "" }), w.saturating_sub(41)))])
        }
        Part::History if i == 0 => Line::from(bold("Approval settings")),
        Part::History => {
            let v = &a.versions[i - 1];
            let when = crate::cmdline::when(v["at"].as_u64());
            Line::from(vec![dim(pad(&format!("v{}", v["version"]), 6)), Span::raw(pad(s(&v["by"]), 12)), dim(pad(&when, 17)), Span::raw(fit(s(&v["what"]), w.saturating_sub(35)))])
        }
        Part::Yours => {
            let o = &a.owned[i];
            let keys = o["tokens"].as_array().map_or(0, Vec::len);
            let state = if o["disabled"] == true { coloured("disabled", WARN) } else { Span::raw("") };
            Line::from(vec![bold(pad(s(&o["name"]), 18)), Span::raw(pad(s(&o["kind"]), 8)), Span::raw(pad(&format!("{keys} key{}", if keys == 1 { "" } else { "s" }), 9)), state])
        }
    });
}

fn draw_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let block = panel("Detail", app.admin.right);
    if app.admin.right {
        super::keys_here(app, area);
    }
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
