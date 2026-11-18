//! The Approvals page's tabs, and its runs waiting at an approval: a list on the left, the picked
//! run's point beside it, with the keys that decide it.

use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{bold, buttons, coloured, dim, fit, keys_here, list, pad, panel, ACCENT, DIM, GOOD, WARN};
use crate::workbench::app::{App, Click, List, Page};
use crate::workbench::approvals::{chain, what, yours, Part, PARTS};

/// The tabs along the top, each with its count: what waits for the caller there.
pub(super) fn draw_parts(f: &mut Frame, app: &App, area: Rect) {
    let mut x = area.x + 1;
    for (p, name) in PARTS {
        let n = match p {
            Part::Deploys => app.waiting().len(),
            Part::Runs => app.runs_waiting().len(),
            Part::Access => app.changes_waiting().len(),
        };
        let here = app.approvals_part == p;
        let label = format!(" {name} {n} ");
        let w = label.chars().count() as u16;
        if x + w > area.x + area.width {
            break;
        }
        let style = if here { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
        let rect = Rect { x, y: area.y, width: w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        f.render_widget(Paragraph::new(Span::styled(if here { "━".repeat(w as usize) } else { String::new() }, Style::new().fg(ACCENT))), Rect { y: area.y + 1, ..rect });
        app.hit(Rect { height: 2, ..rect }, Click::ApprovalPart(p));
        x += w + 1;
    }
    let rule = Rect { x, y: area.y + 1, width: (area.x + area.width).saturating_sub(x), height: 1 };
    f.render_widget(Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::new().fg(DIM))), rule);
}

/// The runs waiting at a point, oldest first; beside them, the picked one's point: its steps as
/// they stand, who decided what and why, what it shows, and until when.
pub(super) fn draw_runs(f: &mut Frame, app: &mut App, area: Rect) {
    let runs = app.runs_waiting();
    let wide = area.width >= 110;
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area) } else { Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area) };
    let block = panel(&format!("Runs waiting ({})", runs.len()), true);
    keys_here(app, left);
    let inner = block.inner(left);
    f.render_widget(block, left);
    if let Some(e) = app.client.as_ref().and_then(|c| c.waiting_runs_error.clone()) {
        return f.render_widget(Paragraph::new(vec![Line::from(coloured(" They can't be read here:", super::BAD)), Line::from(format!(" {e}"))]).wrap(Wrap { trim: false }), inner);
    }
    if runs.is_empty() {
        return f.render_widget(Paragraph::new(vec![Line::from(coloured(" No run waits for a person.", GOOD)), Line::default(), Line::from(dim(" A run that reaches await approval(…) waits here for the people it names."))]), inner);
    }
    app.run_sel = app.run_sel.min(runs.len() - 1);
    let sel = app.run_sel;
    list(f, app, inner, List::Page(Page::Approvals), runs.len(), Some(sel), true, |i, w| {
        let r = &runs[i];
        let point = r["waiting"].as_array().and_then(|p| p.first()).cloned().unwrap_or_default();
        let mark = if yours(r).is_some() { coloured("● ", WARN) } else { dim("◷ ") };
        let head = format!("{} {}", r["slug"].as_str().unwrap_or("?"), crate::decidecmd::short(r["run_id"].as_str().unwrap_or("?")));
        Line::from(vec![mark, bold(pad(&head, 26)), Span::raw(pad(point["name"].as_str().unwrap_or("?"), 12)), dim(fit(&chain(&point), w.saturating_sub(40)))])
    });

    let r = &runs[sel];
    let block = panel(&format!("{}  ·  {} of {}", r["slug"].as_str().unwrap_or("?"), sel + 1, runs.len()), false);
    let inner = block.inner(right);
    f.render_widget(block, right);
    let mut lines = vec![Line::from(vec![dim("Run      "), Span::raw(r["run_id"].as_str().unwrap_or("?").to_string())]), Line::from(vec![dim("What     "), Span::raw(what(r))])];
    lines.push(Line::from(vec![dim("Since    "), Span::raw(crate::cmdline::when(r["parked_at"].as_u64()))]));
    if let Some(d) = r["delivered"].as_array().filter(|d| !d.is_empty()) {
        lines.push(Line::from(vec![dim("Given    "), Span::raw(d.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join(", "))]));
    }
    for w in r["waiting"].as_array().into_iter().flatten() {
        lines.push(Line::default());
        let name = w["name"].as_str().unwrap_or("?");
        let kind = if w["steps"].is_null() { "event" } else { "approval" };
        lines.push(Line::from(vec![bold(format!("{kind} {name:?}")), dim(format!("  until {}", crate::cmdline::when(w["expires_at"].as_u64())))]));
        if let Some(said) = w["said"].as_str() {
            lines.push(Line::from(dim(format!("  {said}"))));
        }
        if !w["steps"].is_null() {
            lines.push(Line::from(format!("  {}", chain(w))));
        }
        for d in w["decisions"].as_array().into_iter().flatten() {
            let who = d["who"].as_str().unwrap_or("?");
            let role = d["role"].as_str().map(|r| format!(" as {r}")).unwrap_or_default();
            let said = if d["approved"] == true { coloured("approved", GOOD) } else { coloured("declined", super::BAD) };
            let why = d["reason"].as_str().map(|r| format!(": {r:?}")).unwrap_or_default();
            lines.push(Line::from(vec![Span::raw(format!("  {who}{role} ")), said, dim(format!(" {}{why}", crate::mcp::when(&d["at"])))]));
        }
        for (k, label) in [("shows", "It shows"), ("input", "Its run was started with")] {
            if !w[k].is_null() {
                lines.push(Line::from(dim(format!("  {label}:"))));
                match &w[k] {
                    serde_json::Value::Object(m) => lines.extend(m.iter().map(|(k, v)| Line::from(format!("    {k}: {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))))),
                    v => lines.push(Line::from(format!("    {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string())))),
                }
            }
        }
    }
    let mine = yours(r).is_some();
    lines.push(Line::default());
    lines.push(Line::from(dim(if mine { "Yours to decide: a approves, d declines (with why)." } else { "None of its steps is yours to take now." })));
    let room = inner.width.saturating_sub(1) as usize;
    let lines: Vec<Line> = lines.into_iter().map(|l| super::fit_line(l, room)).collect();
    f.render_widget(Paragraph::new(lines), inner);
    let mut keys: Vec<(&str, &str, KeyCode, KeyModifiers)> = vec![];
    if mine {
        keys.extend([("a", "approve", KeyCode::Char('a'), KeyModifiers::NONE), ("d", "decline", KeyCode::Char('d'), KeyModifiers::NONE)]);
    }
    keys.extend([("enter", "its flow", KeyCode::Enter, KeyModifiers::NONE), ("tab", "access changes", KeyCode::Tab, KeyModifiers::NONE)]);
    let bar = Rect { x: right.x + 2, y: right.y + right.height - 1, width: right.width.saturating_sub(4), height: 1 };
    buttons(f, app, bar, &keys);
}

/// The access changes waiting for people, newest first; beside them, the picked one: what it
/// changes and gives, who could act as whom it gives more, who proposed it and why, who approved
/// it so far. Under them, the last ten decided, and how.
pub(super) fn draw_access(f: &mut Frame, app: &mut App, area: Rect) {
    use crate::workbench::approvals::{gives, not_yours};
    let all = app.access_changes();
    let waiting: Vec<serde_json::Value> = all.iter().filter(|p| p["state"] == "waiting").cloned().collect();
    let ended: Vec<&serde_json::Value> = all.iter().filter(|p| p["state"] != "waiting").take(10).collect();
    let wide = area.width >= 110;
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area) } else { Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area) };
    // Nothing waits: the whole width for what was decided lately.
    let left = if waiting.is_empty() { area } else { left };
    let block = panel(&format!("Access changes waiting ({})", waiting.len()), true);
    keys_here(app, left);
    let inner = block.inner(left);
    f.render_widget(block, left);
    if let Some(e) = app.client.as_ref().and_then(|c| c.access_changes_error.clone()) {
        return f.render_widget(Paragraph::new(vec![Line::from(coloured(" They can't be shown here:", super::BAD)), Line::from(format!(" {e}"))]).wrap(Wrap { trim: false }), inner);
    }
    let me = app.client.as_ref().and_then(|c| c.me.clone()).unwrap_or_default();
    let may = app.client.as_ref().map(|c| c.may("people.assign", None));
    let said = |p: &serde_json::Value| Line::from(vec![dim(format!("#{} ", p["id"])), Span::raw(p["what"].as_str().unwrap_or("?").to_string()), dim(format!("  {}", crate::accesscmd::proposal_state(p)))]);
    if waiting.is_empty() {
        let mut lines = vec![Line::from(coloured(" No access change waits.", GOOD)), Line::default()];
        lines.push(Line::from(dim(" While this host has people approve an access change (holon access approvals --changes N), one that gives more waits here.")));
        if !ended.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(bold(" Decided lately")));
            lines.extend(ended.iter().map(|p| said(p)));
        }
        let room = inner.width.saturating_sub(1) as usize;
        let lines: Vec<Line> = lines.into_iter().map(|l| super::fit_line(l, room)).collect();
        return f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
    app.change_sel = app.change_sel.min(waiting.len() - 1);
    let sel = app.change_sel;
    list(f, app, inner, List::Page(Page::Approvals), waiting.len(), Some(sel), true, |i, w| {
        let p = &waiting[i];
        let mark = if not_yours(p, &me).is_none() && matches!(may, Some(Ok(()))) { coloured("● ", WARN) } else { dim("◷ ") };
        Line::from(vec![mark, bold(pad(&format!("#{}", p["id"]), 6)), Span::raw(fit(p["what"].as_str().unwrap_or("?"), w.saturating_sub(10)))])
    });

    let p = &waiting[sel];
    let block = panel(&format!("Access change {}  ·  {} of {}", p["id"], sel + 1, waiting.len()), false);
    let inner = block.inner(right);
    f.render_widget(block, right);
    let mut lines = vec![Line::from(bold(p["what"].as_str().unwrap_or("?").to_string())), Line::from(format!("gives {}", gives(p)))];
    let ways: Vec<&str> = p["ways_in"].as_array().into_iter().flatten().filter_map(|w| w.as_str()).collect();
    if !ways.is_empty() {
        lines.push(Line::from(dim(format!("who could act as whom it gives more, and so doesn't approve it: {}", ways.join("; ")))));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![dim("Proposed "), Span::raw(format!("{}, {}", p["author"].as_str().unwrap_or("?"), crate::cmdline::when(p["at"].as_u64())))]));
    if let Some(why) = p["why"].as_str().filter(|w| !w.trim().is_empty()) {
        lines.push(Line::from(vec![dim("Why      "), Span::raw(why.to_string())]));
    }
    lines.push(Line::from(vec![dim("Stands   "), Span::raw(crate::accesscmd::proposal_state(p))]));
    lines.push(Line::default());
    let author = p["author"].as_str() == Some(me.as_str());
    let yours = match (&may, not_yours(p, &me)) {
        (Some(Err(why)), _) => format!("Deciding it takes people.assign: {why}"),
        (_, Some(why)) if author => format!("Not yours to approve: {why}. d withdraws it, saying why."),
        (_, Some(why)) => format!("Not yours to approve: {why}. d declines it, saying why."),
        _ => "Yours to decide: a approves, d declines (with why).".into(),
    };
    lines.push(Line::from(dim(yours)));
    if !ended.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(bold("Decided lately")));
        lines.extend(ended.iter().map(|p| said(p)));
    }
    let room = inner.width.saturating_sub(1) as usize;
    let lines: Vec<Line> = lines.into_iter().map(|l| super::fit_line(l, room)).collect();
    f.render_widget(Paragraph::new(lines), inner);
    let mut keys: Vec<(&str, &str, KeyCode, KeyModifiers)> = vec![];
    if matches!(may, Some(Ok(()))) {
        if !author && not_yours(p, &me).is_none() {
            keys.push(("a", "approve", KeyCode::Char('a'), KeyModifiers::NONE));
        }
        keys.push(("d", if author { "withdraw" } else { "decline" }, KeyCode::Char('d'), KeyModifiers::NONE));
    }
    keys.push(("tab", "deploys", KeyCode::Tab, KeyModifiers::NONE));
    let bar = Rect { x: right.x + 2, y: right.y + right.height - 1, width: right.width.saturating_sub(4), height: 1 };
    buttons(f, app, bar, &keys);
}
