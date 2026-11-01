//! The dialogs: approving a deploy (and what it changes), going back to a point and its dry run,
//! the call form, the palette, a line typed in, a yes or no, the keys. Each has its buttons at
//! the bottom, to press or click.

use holon_author::agent;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::{bold, chips, coloured, dim, fit, list, pad, panel, scrollbar, tone, ACCENT, GOOD, WARN};
use crate::workbench::app::{App, List, Modal, Said, Tone};
use crate::workbench::client::Client;
use crate::workbench::flow::restore_options;
use crate::workbench::palette;

const NONE: KeyModifiers = KeyModifiers::NONE;

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

const HELP: &[(&str, &str)] = &[
    ("Anywhere", ""),
    ("ctrl+p", "find anything: a flow, a page, a sample, a command"),
    ("1 … 7", "overview, approvals, flows, the open flow, the editor, the library, admin"),
    ("ctrl+n", "a new draft in the editor; its first save asks for a file"),
    ("↑ ↓  enter", "pick a row, and open it; a click picks, a second click opens"),
    ("r", "read the server again (it is read every few seconds too)"),
    (":", "a command: open <file|slug>, new [sample], w [file], deploy, flow <slug>,"),
    ("", "call [slug], restore [slug], sessions [slug], run|pause|stop <slug>, server <port|url>, q, q!"),
    ("! or ctrl+`", "the shell line (bottom right): a command runs in this terminal, then its prompt"),
    ("", "stays for more, until esc; cd stays; ↑ the commands before; :!<command> starts at once"),
    ("", ""),
    ("A flow", ""),
    ("← →", "the dashboard: its sections; elsewhere: the list, then the tabs beside it"),
    ("tab", "the next section: status, runs, history, state, access, graph"),
    ("enter", "a run: step through it; history: go back to it (options, a dry run, then it)"),
    ("", "graph: edit the statement picked; a shows every node, or the statements again"),
    ("b", "go back to the run or version picked"),
    ("c  e", "call it; edit its text"),
    ("g  p  x  D", "run, pause, stop or remove it"),
    ("", ""),
    ("Admin", ""),
    ("tab", "the next part: people, API keys, folders, databases, secrets, the server"),
    ("n", "invite someone, make an API key, bind a folder or a database (holon's own flags)"),
    ("enter", "people: the role (e the email, x disable, s a reset link); databases: their tables;"),
    ("", "secrets: the flow's access; server: reload holon.toml, sign-in, sweep the store or folders"),
    ("del", "remove a person, revoke a key, unbind a folder or a database"),
    ("", ""),
    ("Editor", ""),
    ("esc / i", "leave the text for the page keys, and come back to it"),
    ("ctrl+s  ctrl+d", "save (to its file, else on the server); deploy, approving what it asks"),
    ("ctrl+b", "code, then code and its graph side by side, then the graph alone (enter edits there)"),
    ("ctrl+r", "a run over its text: what it played lit, the rest dimmed, each value under its line;"),
    ("", "← → its timeline's steps, marked in the code and the graph; ↑ ↓ the run; b the layout"),
    ("ctrl+z  ctrl+y", "undo, redo"),
    ("shift+arrows", "select; ctrl+a all; ctrl+c ctrl+x ctrl+v copy, cut, paste"),
    ("ctrl+f", "find; enter the next, shift+enter the one before"),
    ("ctrl+space", "what may come here (it is offered as you type a name)"),
    ("ctrl+/", "comment the lines out, or back in"),
    ("alt+↑ alt+↓", "move the lines; with shift, copy them"),
    ("tab  shift+tab", "indent, outdent"),
    ("alt+f", "format, as holon fmt writes it"),
    ("ctrl+pgup pgdn", "the tab before or after; ctrl+w closes this one"),
    ("", ""),
    ("The mouse", ""),
    ("click", "a page, a tab, a row, a button; in the text, the cursor (shift+click selects)"),
    ("wheel", "scrolls what is under it"),
];

/// A dialog's frame: cleared, bordered, its buttons along the bottom; the space inside.
fn frame(f: &mut Frame, app: &App, rect: Rect, title: &str, buttons: &[(&str, &str, KeyCode, KeyModifiers)]) -> Rect {
    f.render_widget(Clear, rect);
    let block = panel(title, true);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if buttons.is_empty() {
        return inner;
    }
    let [body, _, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    chips(f, app, bar, buttons);
    body
}

fn said(lines: &[Said], width: usize) -> Vec<Line<'static>> {
    let mut out = vec![];
    for (i, (t, s)) in lines.iter().enumerate() {
        if *t == Tone::Head && i > 0 {
            out.push(Line::default());
        }
        out.push(Line::from(Span::styled(fit(s, width), tone(*t))));
    }
    out
}

/// Lines that scroll: as many as fit from `scroll`, kept in range.
fn scrolled(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>, scroll: &mut usize) {
    let max = lines.len().saturating_sub(area.height as usize);
    *scroll = (*scroll).min(max);
    let n = lines.len();
    f.render_widget(Paragraph::new(lines).scroll((*scroll as u16, 0)), area);
    scrollbar(f, area, n, *scroll);
}

pub(super) fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let server = app.client.as_ref().map(Client::server).unwrap_or_default();
    // Taken out while drawn, so the dialog can keep its scroll in range; put back after.
    let Some(mut modal) = app.modal.take() else { return };
    match &mut modal {
        Modal::Approve { slug, plan, staged, diff, show_diff, scroll, .. } => {
            let (added, removed) = crate::workbench::diff::counts(diff);
            let mut lines: Vec<Line> = vec![Line::from(dim(format!("Deploying \"{slug}\" ({}) to {server}.", &plan.unit[..12])))];
            if *show_diff {
                lines.push(Line::from(vec![bold("What it changes in the text deployed now  "), coloured(format!("+{added}"), GOOD), dim(" "), coloured(format!("−{removed}"), super::BAD)]));
                for (k, l) in crate::workbench::diff::hunks(diff, 2) {
                    let (mark, style) = match k {
                        '+' => ("+ ", Style::new().fg(GOOD)),
                        '-' => ("- ", Style::new().fg(super::BAD)),
                        '…' => ("  ", Style::new().fg(super::DIM)),
                        _ => ("  ", Style::new()),
                    };
                    lines.push(Line::from(Span::styled(format!("{mark}{}", if k == '…' { "…" } else { &l }), style)));
                }
            } else {
                lines.push(Line::default());
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
                    lines.push(Line::from(coloured(format!("Not set yet: {} (a run that reads one gets nothing until it is set).", staged.missing_secrets.join(", ")), WARN)));
                }
                if !plan.routes.is_empty() {
                    lines.push(Line::default());
                    lines.extend(plan.routes.iter().map(|r| Line::from(dim(format!("  {r}")))));
                }
                if !diff.is_empty() {
                    lines.push(Line::default());
                    lines.push(Line::from(vec![dim("It changes the text deployed now: "), coloured(format!("+{added} "), GOOD), coloured(format!("−{removed}"), super::BAD), dim(" lines (d shows them)")]));
                }
            }
            let title = if staged.needs.is_empty() { format!("Deploy {slug}?") } else { format!("Approve and deploy {slug}?") };
            let go = if staged.needs.is_empty() { "deploy" } else { "approve and deploy" };
            let mut buttons = vec![("enter", go, KeyCode::Enter, NONE)];
            if !diff.is_empty() {
                buttons.push(("d", if *show_diff { "what it may do" } else { "what it changes" }, KeyCode::Char('d'), NONE));
            }
            buttons.push(("esc", "cancel", KeyCode::Esc, NONE));
            let height = (lines.len() as u16 + 4).max(8);
            let rect = centred(area, 90, height);
            let body = frame(f, app, rect, &title, &buttons);
            app.hit(body, crate::workbench::app::Click::Scroll(List::Dialog));
            scrolled(f, body, lines, scroll);
        }
        Modal::Lines { title, lines, scroll } => {
            let rect = centred(area, 100, lines.len() as u16 + 4);
            let body = frame(f, app, rect, &fit(title, 70), &[("esc", "close", KeyCode::Esc, NONE)]);
            let shown = said(lines, body.width as usize);
            scrolled(f, body, shown, scroll);
        }
        Modal::Confirm { title, lines, .. } => {
            let text: Vec<Line> = lines.iter().map(|l| Line::from(l.clone())).collect();
            let wrapped: u16 = lines.iter().map(|l| (l.chars().count() as u16 / 60) + 1).sum();
            let rect = centred(area, 66, wrapped + 5);
            let body = frame(f, app, rect, title, &[("enter", "yes", KeyCode::Enter, NONE), ("esc", "no", KeyCode::Esc, NONE)]);
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), body);
        }
        Modal::Help { scroll } => {
            let lines: Vec<Line> = HELP.iter().map(|(k, what)| if what.is_empty() && !k.is_empty() { Line::from(bold(k.to_string())) } else { Line::from(vec![coloured(format!("  {k:<16}"), ACCENT), Span::raw(what.to_string())]) }).collect();
            let rect = centred(area, 96, lines.len() as u16 + 4);
            let body = frame(f, app, rect, "Keys", &[("esc", "close", KeyCode::Esc, NONE)]);
            app.hit(body, crate::workbench::app::Click::Scroll(List::Dialog));
            scrolled(f, body, lines, scroll);
        }
        Modal::Palette { query, sel } => {
            let items = palette::items(app, query);
            *sel = (*sel).min(items.len().saturating_sub(1));
            let height = (items.len().min(14) as u16 + 4).max(6);
            let rect = Rect { y: area.y + 2, ..centred(area, 76, height) };
            f.render_widget(Clear, rect);
            let block = panel("Find anything", true);
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let [input, rule, rows] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            f.render_widget(Paragraph::new(Line::from(vec![coloured("› ", ACCENT), Span::raw(query.clone()), Span::styled(" ", Style::new().bg(Color::Gray)), dim(if query.is_empty() { "  a flow, a page, a sample, deploy, call…" } else { "" })])), input);
            f.render_widget(Paragraph::new(dim("─".repeat(rule.width as usize))), rule);
            if items.is_empty() {
                f.render_widget(Paragraph::new(dim(" Nothing matches.")), rows);
            } else {
                list(f, app, rows, List::Palette, items.len(), Some(*sel), true, |i, w| {
                    let it = &items[i];
                    Line::from(vec![Span::raw(pad(&it.label, w.saturating_sub(10))), dim(format!("{:>9}", it.kind))])
                });
            }
        }
        Modal::Restore { point, sel, .. } => {
            let options = restore_options(point);
            let rect = centred(area, 78, options.len() as u16 * 2 + 7);
            let body = frame(f, app, rect, &format!("Go back to {}", point.name), &[("enter", "dry run", KeyCode::Enter, NONE), ("esc", "cancel", KeyCode::Esc, NONE)]);
            let [about, rows] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(body);
            f.render_widget(Paragraph::new(dim("What to put back? A dry run says what it changes; nothing changes until you confirm it.")).wrap(Wrap { trim: false }), about);
            list(f, app, rows, List::Restore, options.len(), Some(*sel), true, |i, w| {
                let (what, _, _, ok) = &options[i];
                match ok {
                    Ok(()) => Line::from(bold(fit(what, w))),
                    Err(why) => Line::from(vec![dim(what.clone()), dim(fit(&format!("  ({why})"), w.saturating_sub(what.chars().count())))]),
                }
            });
        }
        Modal::DryRun { point, lines, refused, scroll, needs, .. } => {
            let buttons: Vec<(&str, &str, KeyCode, KeyModifiers)> = if *refused {
                vec![("esc", "close", KeyCode::Esc, NONE)]
            } else if needs.is_empty() {
                vec![("enter", "restore it", KeyCode::Enter, NONE), ("esc", "leave it as it is", KeyCode::Esc, NONE)]
            } else {
                vec![("enter", "approve and restore it", KeyCode::Enter, NONE), ("esc", "leave it as it is", KeyCode::Esc, NONE)]
            };
            let rect = centred(area, 100, lines.len() as u16 + 6);
            let body = frame(f, app, rect, &format!("Dry run: going back to {}", point.name), &buttons);
            let [shown, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(body);
            app.hit(shown, crate::workbench::app::Click::Scroll(List::Dialog));
            scrolled(f, shown, said(lines, shown.width as usize), scroll);
            f.render_widget(Paragraph::new(coloured(if *refused { "It can't be done now." } else { "Nothing has changed yet." }, if *refused { super::BAD } else { WARN })), foot);
        }
        Modal::Call(form) => {
            let rect = centred(area, 104, 24.max(form.answer.len() as u16 + 10).min(36));
            let body = frame(f, app, rect, &format!("Call {}", form.slug), &[("enter", "send", KeyCode::Enter, NONE), ("tab", "next field", KeyCode::Tab, NONE), ("esc", "close", KeyCode::Esc, NONE)]);
            let [left, right] = Layout::horizontal([Constraint::Length(30), Constraint::Min(20)]).areas(body);
            let items: Vec<String> = form.items.iter().map(|i| i.label()).collect();
            list(f, app, left, List::Call, items.len(), Some(form.sel), form.field == 0, |i, w| Line::from(fit(&items[i], w)));
            let n = form.fields();
            let mut constraints = vec![Constraint::Length(1)];
            constraints.extend((0..n).map(|_| Constraint::Length(2)));
            constraints.push(Constraint::Min(1));
            let parts = Layout::vertical(constraints).split(Rect { x: right.x + 2, width: right.width.saturating_sub(2), ..right });
            let what = match &form.items[form.sel] {
                crate::workbench::flow::Callable::Route { .. } => "A request to its route, as anyone calling it would send.",
                crate::workbench::flow::Callable::Action(_) => "Runs the action now, with this input, as you.",
                crate::workbench::flow::Callable::Clock(_) => "Fires the clock now, once, as if its time had come.",
            };
            f.render_widget(Paragraph::new(dim(what)), parts[0]);
            for i in 1..=n {
                let focused = form.field == i;
                let value = form.field_value(i);
                let line = Line::from(vec![
                    Span::styled(format!("{:<13}", form.field_name(i)), if focused { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(super::DIM) }),
                    Span::styled(format!(" {} ", fit(value, parts[i].width.saturating_sub(17) as usize)), Style::new().bg(Color::Indexed(if focused { 238 } else { 235 }))),
                ]);
                f.render_widget(Paragraph::new(line), parts[i]);
                if focused {
                    let x = parts[i].x + 14 + value.chars().count().min(parts[i].width.saturating_sub(17) as usize) as u16;
                    f.set_cursor_position((x, parts[i].y));
                }
            }
            let answer = parts[n + 1];
            if form.answer.is_empty() {
                f.render_widget(Paragraph::new(dim("Its answer shows here, and the run on the flow's page.")), answer);
            } else {
                f.render_widget(Paragraph::new(said(&form.answer, answer.width as usize)).block(panel("Answer", false)), answer);
            }
        }
        Modal::Input { title, lines, value, masked, .. } => {
            let rect = centred(area, 72, lines.len() as u16 + 8);
            let body = frame(f, app, rect, title, &[("enter", "set it", KeyCode::Enter, NONE), ("esc", "cancel", KeyCode::Esc, NONE)]);
            let [about, field] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(body);
            f.render_widget(Paragraph::new(lines.iter().map(|l| Line::from(l.clone())).collect::<Vec<_>>()).wrap(Wrap { trim: false }), about);
            let shown = if *masked { "•".repeat(value.chars().count()) } else { value.clone() };
            let shown = fit(&shown, field.width.saturating_sub(4) as usize);
            f.render_widget(Paragraph::new(Line::from(vec![coloured("› ", ACCENT), Span::styled(format!("{shown} "), Style::new().bg(Color::Indexed(238)))])), field);
            f.set_cursor_position((field.x + 2 + shown.chars().count() as u16, field.y));
        }
    }
    app.modal = Some(modal);
}
