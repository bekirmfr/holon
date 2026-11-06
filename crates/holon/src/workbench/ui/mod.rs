//! Drawing `holon editor`: a bar along the top, the pages down the side, the page, and two lines
//! at the bottom (what just happened, and the keys that work here, each one a button), with a
//! dialog over them. Whatever can be clicked says where it was drawn (`App::hit`).

mod admin;
mod editor;
mod flow;
mod modal;

use holon_author::samples;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::Frame;
use serde_json::{json, Value};

use super::app::{App, Click, List, Page, Show, Tone, PAGES};
use super::flow::Section;

pub(super) const ACCENT: Color = Color::Cyan;
pub(super) const DIM: Color = Color::DarkGray;
/// What a run didn't play: darker than dim, still legible.
pub(super) const DARK: Color = Color::Indexed(240);
pub(super) const WARN: Color = Color::Yellow;
pub(super) const BAD: Color = Color::Red;
pub(super) const GOOD: Color = Color::Green;
/// A picked row's ground, and a quieter one when its list doesn't have the keys.
pub(super) const PICKED: Color = Color::Indexed(237);
pub(super) const PICKED_IDLE: Color = Color::Indexed(235);

/// Below this width the sidebar folds away.
const SIDEBAR_FROM: u16 = 110;
const SIDEBAR: u16 = 21;

pub(super) fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(DIM))
}

pub(super) fn bold(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().add_modifier(Modifier::BOLD))
}

pub(super) fn coloured(s: impl Into<String>, c: Color) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(c))
}

pub(super) fn tone(t: Tone) -> Style {
    match t {
        Tone::Plain => Style::new(),
        Tone::Dim => Style::new().fg(DIM),
        Tone::Head => Style::new().add_modifier(Modifier::BOLD),
        Tone::Good => Style::new().fg(GOOD),
        Tone::Warn => Style::new().fg(WARN),
        Tone::Bad => Style::new().fg(BAD),
        Tone::Added => Style::new().fg(GOOD),
        Tone::Removed => Style::new().fg(BAD),
    }
}

pub(super) fn panel(title: &str, focused: bool) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
        .title(Span::styled(format!(" {title} "), Style::new().add_modifier(Modifier::BOLD)))
}

/// `s` cut to `n` characters, with an ellipsis when cut.
pub(super) fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `s` in `n` characters: cut, or padded with spaces.
pub(super) fn pad(s: &str, n: usize) -> String {
    format!("{:<n$}", fit(s, n))
}

/// The rows from `top` that keep `sel` among `height` shown.
pub(super) fn window(sel: usize, height: usize) -> usize {
    (sel + 1).saturating_sub(height.max(1))
}

/// A list inside `area`: the rows that keep `sel` shown, each one clickable, the picked one
/// marked (brighter when the list has the keys), a scrollbar when they don't all fit.
#[allow(clippy::too_many_arguments)]
pub(super) fn list(f: &mut Frame, app: &App, area: Rect, of: List, n: usize, sel: Option<usize>, focused: bool, row: impl Fn(usize, usize) -> Line<'static>) {
    let height = area.height as usize;
    let top = sel.map_or(0, |s| window(s, height));
    app.hit(area, Click::Scroll(of));
    let width = area.width.saturating_sub(if n > height { 1 } else { 0 }) as usize;
    let mut lines = vec![];
    for i in top..(top + height).min(n) {
        let picked = sel == Some(i);
        let mut spans = vec![Span::styled(if picked { "▌" } else { " " }, Style::new().fg(if focused { ACCENT } else { DIM }))];
        spans.extend(row(i, width.saturating_sub(1)).spans);
        let mut line = Line::from(spans);
        if picked {
            line = line.style(Style::new().bg(if focused { PICKED } else { PICKED_IDLE }));
        }
        lines.push(line);
        app.hit(Rect { x: area.x, y: area.y + (i - top) as u16, width: area.width, height: 1 }, Click::Row(of, i));
    }
    f.render_widget(Paragraph::new(lines), area);
    scrollbar(f, area, n, top);
}

pub(super) fn scrollbar(f: &mut Frame, area: Rect, n: usize, top: usize) {
    if n > area.height as usize {
        let mut state = ScrollbarState::new(n.saturating_sub(area.height as usize) + 1).position(top);
        f.render_stateful_widget(Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None).track_symbol(Some("│")).thumb_symbol("┃").style(Style::new().fg(DIM)), area, &mut state);
    }
}

/// A row of keys' chips from `area`'s left, as many as fit: what to press, then what it does;
/// clicking one presses it. How many fit.
pub(super) fn chips(f: &mut Frame, app: &App, area: Rect, keys: &[(&str, &str, KeyCode, KeyModifiers)]) -> usize {
    let mut x = area.x + 1;
    for (n, (key, what, code, mods)) in keys.iter().enumerate() {
        let k = format!(" {key} ");
        let w = format!(" {what}  ");
        let width = (k.chars().count() + w.chars().count()) as u16;
        if width > (area.x + area.width).saturating_sub(x) {
            return n;
        }
        let rect = Rect { x, y: area.y, width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled(k, Style::new().fg(Color::Black).bg(Color::Gray)), dim(w)])), rect);
        app.hit(rect, Click::Key(*code, *mods));
        x += width;
    }
    keys.len()
}

/// The panel the keys that work now belong to: they show along its bottom edge.
pub(super) fn keys_here(app: &App, rect: Rect) {
    *app.keys_at.borrow_mut() = Some(rect);
}

pub(crate) fn draw(f: &mut Frame, app: &mut App) {
    app.hits.borrow_mut().clear();
    *app.keys_at.borrow_mut() = None;
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
        Page::Flow => flow::draw(f, app, main),
        Page::Editor => editor::draw(f, app, main),
        Page::Library => draw_library(f, app, main),
        Page::Admin => admin::draw(f, app, main),
    }
    draw_note(f, app, note);
    draw_keys(f, app, keys);
    if app.modal.is_some() {
        let area = f.area();
        app.hit(area, Click::Outside);
        modal::draw(f, app, area);
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
        if !c.warnings.is_empty() {
            right.push(coloured(format!("! {} host warning{}", c.warnings.len(), if c.warnings.len() == 1 { "" } else { "s" }), BAD));
            right.push(dim(" · "));
        }
    }
    right.push(Span::raw(format!("{} tab{} ", app.tabs.len(), if app.tabs.len() == 1 { "" } else { "s" })));
    right.push(Span::styled(" ctrl+p ", Style::new().fg(Color::Black).bg(Color::Gray)));
    right.push(dim(" find anything "));
    let right_width: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width as u16)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Line::from(right)), r);
    let palette = Rect { x: r.x + r.width.saturating_sub(23), width: 23.min(r.width), ..r };
    app.hit(palette, Click::Key(KeyCode::Char('p'), KeyModifiers::CONTROL));
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let waiting = app.waiting().len();
    let mut y = area.y + 1;
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
        let rect = Rect { x: area.x, y, width: area.width.saturating_sub(1), height: 1 };
        let line = Line::from(vec![Span::styled(if here { "▌" } else { " " }, Style::new().fg(ACCENT)), dim(format!("{} ", i + 1)), Span::styled(format!("{indent}{label}"), style)]);
        f.render_widget(Paragraph::new(line).style(if here { Style::new().bg(PICKED_IDLE) } else { Style::new() }), rect);
        app.hit(rect, Click::Page(*page));
        y += 1;
    }
    let rest = Rect { y: y + 1, height: (area.y + area.height).saturating_sub(y + 1), ..area };
    let help = vec![Line::from(dim(" ctrl+p  find")), Line::from(dim(" :       command")), Line::from(dim(" ?       keys"))];
    f.render_widget(Paragraph::new(help), rest);
    f.render_widget(Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(DIM)), area);
}

fn draw_note(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(p) = &app.prompt {
        Line::from(vec![coloured(":", ACCENT), Span::raw(p.clone()), Span::styled(" ", Style::new().bg(Color::Gray))])
    } else if let Some(n) = &app.note {
        Line::from(coloured(format!(" {} {}", if n.error { "✗" } else { "✓" }, n.text), if n.error { BAD } else { GOOD }))
    } else if let Some(why) = &app.offline {
        Line::from(coloured(format!(" {}", why.lines().next().unwrap_or("")), WARN))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line), area);
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// The shell line, open: who and where, as a shell's prompt says it, what is typed, and what
/// enter does.
fn draw_shell(f: &mut Frame, area: Rect, line: &str) {
    let lead = format!(" {} ", super::shell::prompt(area.width as usize / 2));
    let hint = "   enter runs it in this terminal, its prompt then stays for more until esc · ↑ earlier";
    let shown = fit(line, (area.width as usize).saturating_sub(lead.chars().count() + 2));
    let spans = vec![Span::styled(lead.clone(), Style::new().fg(Color::Black).bg(GOOD).add_modifier(Modifier::BOLD)), Span::raw(format!(" {shown}")), dim(hint)];
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(Color::Indexed(235))), area);
    f.set_cursor_position((area.x + (lead.chars().count() + 1 + shown.chars().count()) as u16, area.y));
}

/// What ctrl+b shows next, as its key says it.
fn show_next(now: Show) -> &'static str {
    match now.next() {
        Show::Code => "code only",
        Show::Both => "+ graph",
        Show::Graph => "graph only",
    }
}

fn draw_keys(f: &mut Frame, app: &App, area: Rect) {
    use KeyCode::*;
    if let Some(line) = &app.shell {
        return draw_shell(f, area, line);
    }
    let in_flow_pane = app.flow.right;
    let mut keys: Vec<(&str, &str, KeyCode, KeyModifiers)> = if app.modal.is_some() {
        vec![]
    } else if app.prompt.is_some() {
        vec![("enter", "run", Enter, NONE), ("esc", "cancel", Esc, NONE)]
    } else {
        match app.page {
            Page::Editor if app.tabs.is_empty() => vec![("n", "new draft", Char('n'), NONE), ("6", "library", Char('6'), NONE), ("3", "flows", Char('3'), NONE), ("ctrl+p", "find", Char('p'), CTRL), ("q", "quit", Char('q'), NONE)],
            Page::Editor if app.tabs[app.tab].runs.is_some() => vec![("↑↓", "run", Down, NONE), ("→", "next step", Right, NONE), ("←", "step back", Left, NONE), ("tab", "details · errors · timeline", Tab, NONE), ("=", if app.values { "hide values" } else { "show values" }, Char('='), NONE), ("b", show_next(app.show), Char('b'), NONE), ("esc", "back to editing", Esc, NONE)],
            Page::Editor if app.show == Show::Graph => vec![
                ("↑↓", "pick", Down, NONE),
                ("→", "its detail", Right, NONE),
                ("enter", "edit it there", Enter, NONE),
                ("b", show_next(app.show), Char('b'), NONE),
                ("esc", "back to the text", Esc, NONE),
            ],
            Page::Editor if app.beside_has_keys() => vec![("↑↓", "pick", Down, NONE), ("→", "its detail", Right, NONE), ("enter", "edit it there", Enter, NONE), ("b", show_next(app.show), Char('b'), NONE), ("esc", "back to the text", Esc, NONE)],
            Page::Editor if app.find.is_some() => vec![("enter", "next", Enter, NONE), ("shift+enter", "previous", Enter, KeyModifiers::SHIFT), ("esc", "close find", Esc, NONE)],
            Page::Editor if app.complete.is_some() => vec![("tab", "take it", Tab, NONE), ("↑↓", "pick", Down, NONE), ("esc", "close", Esc, NONE)],
            Page::Editor if app.typing => vec![
                ("ctrl+s", "save", Char('s'), CTRL),
                ("ctrl+d", "deploy", Char('d'), CTRL),
                ("ctrl+t", "new tab", Char('t'), CTRL),
                ("ctrl+r", "runs", Char('r'), CTRL),
                ("ctrl+b", show_next(app.show), Char('b'), CTRL),
                ("ctrl+f", "find", Char('f'), CTRL),
                ("ctrl+z", "undo", Char('z'), CTRL),
                ("ctrl+space", "complete", Char(' '), CTRL),
                ("alt+f", "format", Char('f'), KeyModifiers::ALT),
                ("ctrl+w", "close", Char('w'), CTRL),
                ("esc", "page keys", Esc, NONE),
            ],
            Page::Editor => vec![("i", "type", Char('i'), NONE), ("n", "new tab", Char('n'), NONE), ("[ ]", "tabs", Char(']'), NONE), ("s", "save", Char('s'), NONE), ("d", "deploy", Char('d'), NONE), ("v", "runs", Char('v'), NONE), ("b", show_next(app.show), Char('b'), NONE), ("w", "close", Char('w'), NONE), ("q", "quit", Char('q'), NONE)],
            Page::Flow => match app.flow.section {
                Section::Status => vec![("← →", "sections", Right, NONE), ("enter", "go back to a version", Enter, NONE), ("c", "call", Char('c'), NONE), ("e", "edit", Char('e'), NONE), ("g p x", "run pause stop", Char('g'), NONE), ("esc", "flows", Esc, NONE)],
                _ if in_flow_pane => vec![("← →", "tabs", Right, NONE), ("↑↓", "scroll", Down, NONE), ("tab", "next section", Tab, NONE), ("esc", "back to the list", Esc, NONE)],
                Section::Runs => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "step through", Enter, NONE), ("b", "go back to it", Char('b'), NONE), ("c", "call", Char('c'), NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::History => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "go back to it", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::State => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "its history", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::Access => vec![("↑↓", "pick", Down, NONE), ("enter", "change it", Enter, NONE), ("del", "remove a secret", Delete, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::Graph => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "edit it there", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
            },
            Page::Overview => vec![("↑↓", "pick", Down, NONE), ("enter", "open", Enter, NONE), ("e", "edit", Char('e'), NONE), ("c", "call", Char('c'), NONE), ("ctrl+p", "find", Char('p'), CTRL), ("q", "quit", Char('q'), NONE)],
            Page::Approvals => vec![("↑↓", "pick", Down, NONE), ("enter", "review and approve", Enter, NONE), ("q", "quit", Char('q'), NONE)],
            Page::Flows => vec![("↑↓", "pick", Down, NONE), ("enter", "open", Enter, NONE), ("e", "edit", Char('e'), NONE), ("c", "call", Char('c'), NONE), ("ctrl+p", "find", Char('p'), CTRL), ("q", "quit", Char('q'), NONE)],
            Page::Library => vec![("↑↓", "pick", Down, NONE), ("enter", "open a copy", Enter, NONE), ("q", "quit", Char('q'), NONE)],
            Page::Admin if app.admin.right => vec![("↑↓", "scroll", Down, NONE), ("tab", "next part", Tab, NONE), ("esc", "back to the list", Esc, NONE)],
            Page::Admin => {
                use crate::workbench::admin::Part;
                let mut keys = vec![("↑↓", "pick", Down, NONE)];
                keys.extend(match app.admin.part {
                    Part::People => vec![("n", "invite", Char('n'), NONE), ("enter", "role", Enter, NONE), ("e", "email", Char('e'), NONE), ("x", "disable", Char('x'), NONE), ("s", "reset link", Char('s'), NONE), ("del", "remove", Delete, NONE)],
                    Part::Keys => vec![("n", "new key", Char('n'), NONE), ("del", "revoke", Delete, NONE)],
                    Part::Folders => vec![("n", "bind a folder", Char('n'), NONE), ("del", "unbind", Delete, NONE)],
                    Part::Databases => vec![("n", "bind a database", Char('n'), NONE), ("enter", "tables", Enter, NONE), ("del", "unbind", Delete, NONE)],
                    Part::Secrets => vec![("enter", "set them", Enter, NONE)],
                    Part::Server => vec![("enter", "do it", Enter, NONE)],
                });
                keys.extend([("tab", "part", Tab, NONE), ("esc", "overview", Esc, NONE)]);
                keys
            }
        }
    };
    // r reads the server again on every page but the editor's, where it is typed.
    if app.modal.is_none() && app.prompt.is_none() && app.page != Page::Editor {
        keys.insert(keys.len().saturating_sub(1), ("r", "refresh", Char('r'), NONE));
    }
    if app.modal.is_some() || app.prompt.is_some() {
        chips(f, app, area, &keys);
        return;
    }
    // The keys that work in one panel show along its bottom edge; along the bottom of the screen,
    // those that work anywhere (and what didn't fit there).
    let anywhere = |k: &str| matches!(k, "ctrl+p" | "q" | "r");
    let mut bar: Vec<(&str, &str, KeyCode, KeyModifiers)> = keys.iter().filter(|(k, ..)| anywhere(k)).copied().collect();
    if !(app.page == Page::Editor && app.typing) {
        bar.extend([(":", "command", Char(':'), NONE), ("?", "keys", Char('?'), NONE)]);
    }
    let local: Vec<_> = keys.iter().filter(|(k, ..)| !anywhere(k)).copied().collect();
    let shown = match *app.keys_at.borrow() {
        Some(rect) => chips(f, app, Rect { x: rect.x + 1, y: rect.y + rect.height.saturating_sub(1), width: rect.width.saturating_sub(3), height: 1 }, &local),
        None => 0,
    };
    bar.extend_from_slice(&local[shown..]);
    let keys = bar;
    // At the right: the shell line, closed, in what the keys leave (its path shortened to fit,
    // never below a few letters of it). ctrl+` opens it from anywhere, even while typing.
    let used: usize = 1 + keys.iter().map(|(k, w, ..)| k.chars().count() + w.chars().count() + 5).sum::<usize>();
    let left = (area.width as usize).saturating_sub(used).max(22);
    let shell = format!(" {} ", super::shell::prompt(left.saturating_sub(6)));
    let shell_width = (shell.chars().count() as u16 + 4).min(left as u16).min(area.width);
    let [area, right] = Layout::horizontal([Constraint::Min(0), Constraint::Length(shell_width)]).areas(area);
    chips(f, app, area, &keys);
    let r = Rect { x: right.x + 1, width: right.width.saturating_sub(1), ..right };
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" ! ", Style::new().fg(Color::Black).bg(Color::Gray)), Span::styled(fit(&shell, r.width.saturating_sub(3) as usize), Style::new().fg(GOOD).bg(Color::Indexed(235)))])), r);
    app.hit(r, Click::Key(Char('`'), CTRL));
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
pub(super) fn status(d: &Value) -> Span<'static> {
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

/// How a deployment is reached: its port, its clock, or what starts its runs besides a person
/// (its clock handlers, `every 5s`, and file handlers, `on file inbox`).
pub(super) fn reach(d: &Value) -> String {
    let started = d["triggers"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|t| t.starts_with("every ") || t.starts_with("on file ")).collect::<Vec<_>>();
    if let Some(p) = d["port"].as_u64() {
        format!(":{p}")
    } else if let Some(i) = d["interval_seconds"].as_u64() {
        format!("every {i}s")
    } else if !started.is_empty() {
        started.join(", ")
    } else {
        "by hand".into()
    }
}

fn deployment_row(d: &Value, width: usize) -> Line<'static> {
    let slug = d["slug"].as_str().unwrap_or("?");
    let unit = d["unit"].as_str().unwrap_or("");
    let runs = d["trigger_count"].as_u64().unwrap_or(0);
    let failed = d["error_count"].as_u64().unwrap_or(0);
    let st = status(d);
    let pad_st = 11usize.saturating_sub(st.content.chars().count());
    let mut spans = vec![bold(pad(slug, 22)), st, Span::raw(" ".repeat(pad_st)), dim(format!("{:<10}", &unit[..unit.len().min(8)])), Span::raw(format!("{:<10}", reach(d)))];
    spans.push(Span::raw(format!("{runs:>6} run{}", if runs == 1 { " " } else { "s" })));
    spans.push(if failed > 0 { coloured(format!("{failed:>5} failed"), BAD) } else { dim(format!("{:>12}", "")) });
    if width > 90 {
        let by = d["deployed_by"].as_str().map(|b| format!("by {b}, ")).unwrap_or_default();
        spans.push(dim(format!("   {by}{}", crate::mcp::when(&d["deployed_at_epoch_seconds"]))));
    }
    Line::from(spans)
}

/// A number with what it counts under it, in a box.
pub(super) fn tile(f: &mut Frame, area: Rect, label: &str, value: &str, sub: &str, colour: Color) {
    let block = Block::new().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![Line::from(dim(label.to_string())), Line::from(Span::styled(value.to_string(), Style::new().fg(colour).add_modifier(Modifier::BOLD))), Line::from(dim(fit(sub, inner.width as usize)))];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Overview") {
        return;
    }
    let c = app.client.as_ref().unwrap();
    let waiting = app.waiting();
    let running = c.deployments.iter().filter(|d| d["status"] == "running").count();
    let runs: u64 = c.deployments.iter().filter_map(|d| d["trigger_count"].as_u64()).sum();
    let failed: u64 = c.deployments.iter().filter_map(|d| d["error_count"].as_u64()).sum();
    let tiles_height = if area.height >= 20 { 5 } else { 0 };
    // What the host warns of, then what waits for a person, in one panel.
    let warned: Vec<(String, String)> = c.warnings.iter().map(|w| (w["says"].as_str().unwrap_or_default().to_string(), w["todo"].as_str().unwrap_or_default().to_string())).collect();
    let wait_height = (waiting.len().clamp(1, 6) + warned.len().min(3) * 2 + 2) as u16;
    let [tiles, rest] = Layout::vertical([Constraint::Length(tiles_height), Constraint::Min(3)]).areas(area);
    if tiles_height > 0 {
        let parts = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(tiles);
        tile(f, parts[0], "Deployed", &c.deployments.len().to_string(), &format!("{running} running"), ACCENT);
        tile(f, parts[1], "Waiting for you", &waiting.len().to_string(), if waiting.is_empty() { "nothing to approve" } else { "2 opens them" }, if waiting.is_empty() { GOOD } else { WARN });
        tile(f, parts[2], "Runs", &runs.to_string(), "on record, all flows", Color::White);
        tile(f, parts[3], "Failed", &failed.to_string(), &if runs > 0 { format!("{:.1}% of runs", failed as f64 * 100.0 / runs as f64) } else { "no runs yet".into() }, if failed > 0 { BAD } else { GOOD });
    }
    let wide = rest.width >= 120 && !c.activity.is_empty();
    let [left, right] = Layout::horizontal([Constraint::Min(40), Constraint::Length(if wide { 52 } else { 0 })]).areas(rest);
    let [deps, waits] = Layout::vertical([Constraint::Min(4), Constraint::Length(wait_height)]).areas(left);

    let block = panel(&format!("Deployed on {}", c.server()), true);
    let inner = block.inner(deps);
    f.render_widget(block, deps);
    keys_here(app, deps);
    if c.deployments.is_empty() {
        f.render_widget(Paragraph::new(dim("Nothing deployed yet. Open a sample (6), or a file with :open <file>, and deploy it with ctrl+d.")).wrap(Wrap { trim: false }), inner);
    } else {
        list(f, app, inner, List::Page(Page::Overview), c.deployments.len(), Some(app.sel[0]), true, |i, w| deployment_row(&c.deployments[i], w));
    }

    let mut lines: Vec<Line> = warned.iter().take(3).flat_map(|(says, todo)| [Line::from(vec![coloured("! ", BAD), Span::raw(says.clone())]), Line::from(dim(format!("  {todo}")))]).collect();
    if waiting.is_empty() {
        lines.push(Line::from(dim("Nothing waits for a person.")));
    } else {
        lines.extend(waiting.iter().take(6).map(|(slug, w)| Line::from(vec![coloured("● ", WARN), bold(format!("{slug}  ")), Span::raw(w.clone())])));
    }
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Waiting for you ({})", waiting.len()), false)), waits);
    app.hit(waits, Click::Page(Page::Approvals));

    if wide {
        let lines: Vec<Line> = crate::flowscmd::activity_lines(&json!({ "entries": c.activity })).into_iter().map(|l| Line::from(fit(&l, right.width.saturating_sub(2) as usize))).collect();
        f.render_widget(Paragraph::new(lines).block(panel("Lately", false)), right);
    }
}

fn draw_approvals(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Approvals") {
        return;
    }
    let waiting = app.waiting();
    let block = panel("Approvals", true);
    keys_here(app, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [about, rows] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
    f.render_widget(Paragraph::new(dim("What waits for a person: you approve a flow by deploying it, after reading what it may do.")), about);
    if waiting.is_empty() {
        f.render_widget(Paragraph::new(coloured(" Nothing waits for you.", GOOD)), rows);
        return;
    }
    list(f, app, rows, List::Page(Page::Approvals), waiting.len(), Some(app.sel[1]), true, |i, _| Line::from(vec![bold(pad(&waiting[i].0, 22)), Span::raw(waiting[i].1.clone())]));
}

fn draw_flows(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Flows") {
        return;
    }
    let c = app.client.as_ref().unwrap();
    let names = app.flow_names();
    let block = panel(&format!("Flows ({})", names.len()), true);
    keys_here(app, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [head, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    f.render_widget(Paragraph::new(Line::from(vec![dim(format!("  {:<22}{:<11}{:<10}{:<10}", "name", "state", "version", "reach")), dim("saved")])), head);
    if names.is_empty() {
        f.render_widget(Paragraph::new(dim(" No flows on this server yet.")), rows);
        return;
    }
    list(f, app, rows, List::Page(Page::Flows), names.len(), Some(app.sel[2]), true, |i, _| {
        let name = &names[i];
        let saved = c.saved(name).map(|s| {
            let by = s["saved_by"].as_str().map(|b| format!(" by {b}")).unwrap_or_default();
            format!("saved{by}, {}", crate::mcp::when(&s["saved_at_epoch_seconds"]))
        });
        let mut spans = vec![bold(pad(name, 22))];
        match c.deployment(name) {
            Some(d) => {
                let st = status(d);
                let n = 11usize.saturating_sub(st.content.chars().count());
                let unit = d["unit"].as_str().unwrap_or("");
                spans.extend([st, Span::raw(" ".repeat(n)), dim(format!("{:<10}", &unit[..unit.len().min(8)])), Span::raw(format!("{:<10}", reach(d)))]);
            }
            None => spans.push(coloured(format!("{:<31}", "not deployed"), DIM)),
        }
        spans.push(dim(saved.unwrap_or_default()));
        Line::from(spans)
    });
}

fn draw_library(f: &mut Frame, app: &App, area: Rect) {
    let all = samples::samples();
    let sel = app.sel[5].min(all.len().saturating_sub(1));
    let [left, about] = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
    let block = panel(&format!("Samples ({})", all.len()), true);
    keys_here(app, left);
    let inner = block.inner(left);
    f.render_widget(block, left);
    list(f, app, inner, List::Page(Page::Library), all.len(), Some(sel), true, |i, w| Line::from(vec![bold(pad(all[i].name, 20)), dim(fit(all[i].title, w.saturating_sub(20)))]));
    if let Some(s) = all.get(sel) {
        let mut lines = vec![Line::from(bold(s.title.to_string())), Line::default(), Line::from(s.description.to_string()), Line::default()];
        lines.push(Line::from(vec![dim("Shows  "), Span::raw(s.shows.join(", "))]));
        lines.push(Line::default());
        lines.push(Line::from(dim("Try it")));
        lines.extend(s.try_it.lines().map(|l| Line::from(format!("  {l}"))));
        lines.push(Line::default());
        lines.push(Line::from(dim("Enter (or a second click) opens a copy in a tab: it is yours to edit, save and deploy.")));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(s.name, false)), about);
    }
}
