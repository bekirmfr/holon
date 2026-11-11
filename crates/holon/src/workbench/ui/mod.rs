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
use super::client::Client;
use super::flow::Section;
use super::graph::Kind;

pub(super) const ACCENT: Color = Color::Cyan;
pub(super) const DIM: Color = Color::DarkGray;
/// What a run didn't play: darker than dim, still legible.
pub(super) const DARK: Color = Color::Indexed(240);
pub(super) const WARN: Color = Color::Yellow;
pub(super) const BAD: Color = Color::Red;
pub(super) const GOOD: Color = Color::Green;
/// A flow's data (its state, collections, databases), and a call outside it: one colour each,
/// wherever they show (the graph, a run's timeline, the inspector).
pub(super) const DATA: Color = Color::Indexed(141);
pub(super) const OUTSIDE: Color = Color::Indexed(73);
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

/// A kind's colour: its data purple, a call outside teal, else none of its own.
pub(super) fn kind_colour(k: Kind) -> Option<Color> {
    match k {
        Kind::Data => Some(DATA),
        Kind::Outside => Some(OUTSIDE),
        Kind::Compute | Kind::Answer => None,
    }
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
        Tone::Data => Style::new().fg(DATA),
        Tone::Outside => Style::new().fg(OUTSIDE),
    }
}

/// A panel: the one with the keys drawn heavy, its title in the accent; the others thin and dim.
pub(super) fn panel(title: &str, focused: bool) -> Block<'static> {
    let title_style = if focused { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().add_modifier(Modifier::BOLD) };
    Block::new()
        .borders(Borders::ALL)
        .border_type(if focused { BorderType::Thick } else { BorderType::Rounded })
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
        .title(if title.is_empty() { Span::raw("") } else { Span::styled(format!(" {title} "), title_style) })
}

/// A line cut to `width` cells, with an ellipsis when cut.
pub(super) fn fit_line(line: Line<'static>, width: usize) -> Line<'static> {
    let total: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if total <= width {
        return line;
    }
    let mut left = width.saturating_sub(1);
    let mut spans = vec![];
    for s in line.spans {
        let n = s.content.chars().count();
        if n <= left {
            left -= n;
            spans.push(s);
        } else {
            spans.push(Span::styled(s.content.chars().take(left).collect::<String>(), s.style));
            break;
        }
    }
    spans.push(dim("…"));
    Line::from(spans)
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
    chips_led(f, app, area, keys, false)
}

/// A dialog's buttons: as `chips`, the first (what enter does) filled with the accent.
pub(super) fn buttons(f: &mut Frame, app: &App, area: Rect, keys: &[(&str, &str, KeyCode, KeyModifiers)]) -> usize {
    chips_led(f, app, area, keys, keys.first().is_some_and(|(k, ..)| *k == "enter"))
}

fn chips_led(f: &mut Frame, app: &App, area: Rect, keys: &[(&str, &str, KeyCode, KeyModifiers)], lead: bool) -> usize {
    let mut x = area.x + 1;
    for (n, (key, what, code, mods)) in keys.iter().enumerate() {
        let k = format!(" {key} ");
        let w = format!(" {what}  ");
        let width = (k.chars().count() + w.chars().count()) as u16;
        if width > (area.x + area.width).saturating_sub(x) {
            return n;
        }
        let rect = Rect { x, y: area.y, width, height: 1 };
        let primary = lead && n == 0;
        let chip = if primary { Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Black).bg(Color::Gray) };
        let label = if primary { Span::styled(w, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)) } else { dim(w) };
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled(k, chip), label])), rect);
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
    suggest(f, app, body);
    draw_keys(f, app, keys);
    if app.modal.is_some() {
        let area = f.area();
        // What is behind a dialog goes quiet, so the dialog is what's read.
        for cell in f.buffer_mut().content.iter_mut() {
            cell.set_style(Style::new().fg(DIM).bg(Color::Reset).remove_modifier(Modifier::BOLD | Modifier::UNDERLINED));
        }
        app.hit(area, Click::Outside);
        modal::draw(f, app, area);
    }
    if app.light {
        for cell in f.buffer_mut().content.iter_mut() {
            let (fg, bg) = (cell.fg, cell.bg);
            cell.set_fg(light_fg(fg, bg));
            cell.set_bg(light_bg(bg));
        }
    }
}

/// On a light terminal, a ground drawn dark: as light as it was dark, its tint kept.
fn light_bg(c: Color) -> Color {
    match c {
        Color::Indexed(n @ 232..=241) => Color::Indexed(255 - (n - 232)),
        Color::Rgb(r, g, b) => {
            let lift = 255 - r.max(g).max(b);
            Color::Rgb(r + lift, g + lift, b + lift)
        }
        c => c,
    }
}

/// On a light terminal, text drawn light: dark enough to read. Text on a coloured chip stays.
fn light_fg(c: Color, bg: Color) -> Color {
    let on_chip = matches!(bg, Color::Cyan | Color::Green | Color::Yellow | Color::Red | Color::Gray | Color::Magenta);
    match c {
        _ if on_chip => c,
        Color::White => Color::Black,
        Color::Gray => Color::Indexed(240),
        Color::Indexed(n @ 238..=246) => Color::Indexed(n.saturating_sub(4).max(236)),
        Color::Indexed(180) => Color::Indexed(130),
        Color::Yellow => Color::Indexed(130),
        Color::Cyan => Color::Indexed(31),
        Color::Rgb(r, g, b) => Color::Rgb(r / 2, g / 2, b / 2),
        c => c,
    }
}

fn draw_top(f: &mut Frame, app: &App, area: Rect) {
    let mut left = vec![Span::styled(" holon ", Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)), Span::raw(" ")];
    let mut role = None;
    match (&app.client, &app.offline) {
        (Some(c), None) => {
            left.push(coloured("● ", GOOD));
            left.push(Span::raw(c.server()));
            if let Some(me) = &c.me {
                role = Some(format!("{me}{}", c.role.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()));
            }
        }
        (Some(c), Some(_)) => {
            left.push(coloured("● ", BAD));
            left.push(Span::raw(c.server()));
            left.push(coloured("  can't reach it", BAD));
        }
        (None, _) => left.push(coloured("○ no server: editing files only", WARN)),
    }
    // Each count on the right goes to the page that holds what it counts.
    let mut right: Vec<(Span<'static>, Option<Page>)> = vec![];
    let has_role = role.is_some();
    if let Some(who) = role {
        right.push((dim(who), None));
        right.push((dim(" · "), None));
    }
    if let Some(c) = &app.client {
        right.push((Span::raw(format!("{} deployed", c.deployments.len())), Some(Page::Flows)));
        let waiting = app.waiting().len();
        right.push((dim(" · "), None));
        right.push((if waiting > 0 { coloured(format!("● {waiting} waiting for you"), WARN) } else { dim("nothing waiting") }, Some(Page::Approvals)));
        let paused = c.deployments.iter().filter(|d| d["status"] == "paused").count();
        if paused > 0 {
            right.push((dim(" · "), None));
            right.push((coloured(format!("● {paused} paused"), BAD), Some(Page::Flows)));
        }
        right.push((dim(" · "), None));
        if !c.warnings.is_empty() {
            right.push((coloured(format!("! {} host warning{}", c.warnings.len(), if c.warnings.len() == 1 { "" } else { "s" }), BAD), Some(Page::Overview)));
            right.push((dim(" · "), None));
        }
    }
    right.push((Span::raw(format!("{} tab{} ", app.tabs.len(), if app.tabs.len() == 1 { "" } else { "s" })), Some(Page::Editor)));
    right.push((Span::styled(" ctrl+p ", Style::new().fg(Color::Black).bg(Color::Gray)), None));
    right.push((dim(" find anything "), None));
    right.push((dim(format!("· {} ", clock())), None));
    // Narrow, the bar gives way, so the server's name stays whole and where you are shows:
    // first what ctrl+p does, then the clock, who you are, the tabs, the deployed count and the
    // chip; only then the crumbs, the first going before the flow or tab they lead to.
    let parts = crumbs(app);
    let width = |r: &[(Span, Option<Page>)]| r.iter().map(|(s, _)| s.content.chars().count()).sum::<usize>();
    let base: usize = left.iter().map(|s| s.content.chars().count()).sum::<usize>() + 2;
    let fits = |shown: &[(Span, Option<Click>)], right: &[(Span, Option<Page>)]| base + shown.iter().map(|(s, _)| s.content.chars().count()).sum::<usize>() + width(right) <= area.width as usize;
    let full = crumb_spans(&parts, 0);
    let at = |right: &[(Span, Option<Page>)], text: &str| right.iter().position(|(s, _)| s.content.contains(text));
    for gone in [" find anything ", "· "] {
        if !fits(&full, &right) && let Some(i) = right.iter().position(|(s, _)| s.content.starts_with(gone) && (gone != "· " || s.content.ends_with("Z "))) {
            right.remove(i);
        }
    }
    // Who you are, and the dot after it.
    if has_role && !fits(&full, &right) {
        right.drain(..2);
    }
    // The tabs, the dot before them a space keeping ctrl+p apart.
    if !fits(&full, &right) && let Some(i) = at(&right, " tab") {
        right.remove(i);
        if let Some(dot) = i.checked_sub(1).filter(|&d| right[d].0.content == " · ") {
            right[dot].0 = Span::raw(" ");
        }
    }
    // Then the deployed count (the flows page has them), and its dot.
    if !fits(&full, &right) && let Some(i) = at(&right, " deployed") {
        right.drain(i..(i + 2).min(right.len()));
    }
    // Then the ctrl+p chip (ctrl+p works all the same), and the space before it.
    if !fits(&full, &right) && let Some(i) = at(&right, " ctrl+p ") {
        right.remove(i);
        if i > 0 && right[i - 1].0.content == " " {
            right.remove(i - 1);
        }
    }
    let shown = (0..3).map(|cut| crumb_spans(&parts, cut)).find(|c| fits(c, &right)).unwrap_or_else(|| crumb_spans(&parts, 2));
    let chip = right.iter().position(|(s, _)| s.content == " ctrl+p ");
    let right_width = width(&right).min(area.width.saturating_sub(base as u16) as usize);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width as u16)]).areas(area);
    // A crumb before the last goes back to it.
    let mut x = l.x + base as u16 - 2;
    for (span, click) in &shown {
        let w = span.content.chars().count() as u16;
        if let Some(click) = click {
            app.hit(Rect { x, width: w.min(l.right().saturating_sub(x)), ..l }, *click);
        }
        x += w;
    }
    left.extend(shown.into_iter().map(|(s, _)| s));
    f.render_widget(Paragraph::new(Line::from(left)), l);
    let mut x = r.x;
    for (i, (span, to)) in right.iter().enumerate() {
        let w = span.content.chars().count() as u16;
        if let Some(page) = to {
            app.hit(Rect { x, width: w, ..r }, Click::Page(*page));
        }
        if Some(i) == chip {
            app.hit(Rect { x, width: w + right.get(i + 1).filter(|(s, _)| s.content.contains("find")).map_or(0, |(s, _)| s.content.chars().count() as u16), ..r }, Click::Key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        }
        x += w;
    }
    f.render_widget(Paragraph::new(Line::from(right.into_iter().map(|(s, _)| s).collect::<Vec<_>>())), r);
}

/// The crumbs after the server's name, each with what a click on it does: all of them
/// (`cut` 0), the first given up for `…` (1: `… › refunds › Runs`), or the last alone (2).
fn crumb_spans(parts: &[(String, Option<Click>)], cut: usize) -> Vec<(Span<'static>, Option<Click>)> {
    let sep = || (Span::styled(" › ", Style::new().fg(DARK)), None);
    let n = parts.len();
    let kept: Vec<usize> = match cut {
        _ if n <= 1 => (0..n).collect(),
        0 => (0..n).collect(),
        1 if n > 2 => vec![usize::MAX, n - 2, n - 1],
        1 => (0..n).collect(),
        _ => vec![n - 1],
    };
    let mut out = vec![];
    for i in kept {
        out.push(sep());
        if i == usize::MAX {
            out.push((dim("…"), None));
        } else if i == n - 1 {
            out.push((bold(parts[i].0.clone()), None));
        } else {
            out.push((dim(parts[i].0.clone()), parts[i].1));
        }
    }
    out
}

/// The time now, as the server's records write it: `09:44Z`.
fn clock() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default() % 86_400;
    format!("{:02}:{:02}Z", s / 3600, s / 60 % 60)
}

/// The pages down the side, in two groups (the work, then the host), each with its count on the
/// right; under them, how the server is, and the keys that work anywhere.
fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let waiting = app.waiting().len();
    let width = area.width.saturating_sub(1) as usize;
    let mut y = area.y + 1;
    let head = |f: &mut Frame, y: u16, name: &str| f.render_widget(Paragraph::new(Span::styled(format!("  {name}"), Style::new().fg(DARK).add_modifier(Modifier::BOLD))), Rect { x: area.x, y, width: area.width.saturating_sub(1), height: 1 });
    head(f, y, "WORK");
    y += 1;
    for (i, (page, name)) in PAGES.iter().enumerate() {
        if *page == Page::Admin {
            y += 1;
            head(f, y, "HOST");
            y += 1;
        }
        let here = app.page == *page;
        let label = match page {
            Page::Flow => match &app.detail {
                Some(d) => format!(" └ {}", fit(&d.slug, 11)),
                None => continue,
            },
            _ => name.to_string(),
        };
        let (count, count_style) = match page {
            Page::Approvals if waiting > 0 => (waiting.to_string(), Style::new().fg(WARN).add_modifier(Modifier::BOLD)),
            Page::Flows => (app.all_flow_names().len().to_string(), Style::new().fg(DIM)),
            Page::Editor if !app.tabs.is_empty() => (app.tabs.len().to_string(), Style::new().fg(DIM)),
            _ => (String::new(), Style::new()),
        };
        let style = if here { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new() };
        let rect = Rect { x: area.x, y, width: area.width.saturating_sub(1), height: 1 };
        let used = 4 + label.chars().count();
        let gap = width.saturating_sub(used + count.chars().count() + 1);
        let line = Line::from(vec![Span::styled(if here { "▌" } else { " " }, Style::new().fg(ACCENT)), Span::styled(format!("{} ", i + 1), Style::new().fg(if here { ACCENT } else { DIM })), Span::raw(" "), Span::styled(label, style), Span::raw(" ".repeat(gap)), Span::styled(count, count_style)]);
        f.render_widget(Paragraph::new(line).style(if here { Style::new().bg(PICKED) } else { Style::new() }), rect);
        app.hit(rect, Click::Page(*page));
        y += 1;
    }
    let bottom = area.y + area.height;
    // How the server is, at the foot: what runs, what's paused, what it warns of.
    let mut health = vec![Line::from(Span::styled("  SERVER", Style::new().fg(DARK).add_modifier(Modifier::BOLD)))];
    match (&app.client, &app.offline) {
        (Some(c), None) => {
            let running = c.deployments.iter().filter(|d| d["status"] == "running").count();
            let paused = c.deployments.iter().filter(|d| d["status"] == "paused").count();
            health.push(Line::from(vec![coloured("  ● ", GOOD), dim(format!("{running} running"))]));
            if paused > 0 {
                health.push(Line::from(vec![coloured("  ● ", WARN), dim(format!("{paused} paused"))]));
            }
            if !c.warnings.is_empty() {
                health.push(Line::from(vec![coloured("  ! ", BAD), dim(format!("{} warning{}", c.warnings.len(), if c.warnings.len() == 1 { "" } else { "s" }))]));
            }
            // The run logs' chains, as last read: intact, or where a line was changed outside.
            let broken: usize = c.chains.values().map(|(b, _)| *b).sum();
            let on_record: u64 = c.deployments.iter().filter_map(|d| d["trigger_count"].as_u64()).sum();
            health.push(if broken == 0 { Line::from(vec![coloured("  ✓ ", GOOD), dim("run logs intact")]) } else { Line::from(vec![coloured("  ✗ ", BAD), coloured(format!("{broken} break{} in the logs", if broken == 1 { "" } else { "s" }), BAD)]) });
            // Under "run logs intact": its runs, as many as a narrow column holds (12.3k, 4.5M).
            let n = match on_record {
                0..10_000 => on_record.to_string(),
                10_000..1_000_000 => format!("{:.1}k", on_record as f64 / 1e3),
                _ => format!("{:.1}M", on_record as f64 / 1e6),
            };
            health.push(Line::from(dim(format!("    {n} on record"))));
        }
        (Some(_), Some(_)) => health.push(Line::from(vec![coloured("  ● ", BAD), dim("can't reach it")])),
        (None, _) => health.push(Line::from(vec![coloured("  ○ ", WARN), dim("none: files only")])),
    }
    let help = [Line::default(), Line::from(dim(" ctrl+p  find")), Line::from(dim(" :       command")), Line::from(dim(" ?       keys"))];
    let need = (health.len() + help.len()) as u16;
    if bottom > y + need {
        let room = area.width.saturating_sub(1) as usize;
        let lines: Vec<Line> = health.into_iter().chain(help).map(|l| fit_line(l, room)).collect();
        f.render_widget(Paragraph::new(lines), Rect { y: bottom - need, height: need, ..area });
    } else if bottom > y + 4 {
        f.render_widget(Paragraph::new(help.to_vec()), Rect { y: y + 1, height: bottom.saturating_sub(y + 1), ..area });
    }
    f.render_widget(Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(DIM)), area);
}

fn draw_note(f: &mut Frame, app: &App, area: Rect) {
    let typing = |lead: &'static str, text: &str, hint: &'static str| {
        let room = area.width.saturating_sub(2);
        let (spans, col, from) = typed(text, app.line_back, room as usize);
        (Line::from([vec![coloured(lead, ACCENT)], spans, vec![dim(hint)]].concat()), col, from, room)
    };
    let open = if let Some(p) = &app.prompt {
        Some(typing(":", p, ""))
    } else if app.filtering {
        Some(typing("/", app.filter_of(app.page), "   enter keeps it · esc clears it · ↑ ↓ pick"))
    } else {
        None
    };
    if let Some((line, col, from, room)) = open {
        f.render_widget(Paragraph::new(line), area);
        // Behind a dialog it is drawn, but the dialog has the cursor.
        if app.modal.is_none() {
            typed_at(f, app, area.x + 1, area.y, room, col, from);
        }
        return;
    }
    let line = if let Some(n) = &app.note {
        if n.quiet {
            Line::from(dim(format!(" · {}", n.text)))
        } else {
            Line::from(coloured(format!(" {} {}", if n.error { "✗" } else { "✓" }, n.text), if n.error { BAD } else { GOOD }))
        }
    } else if let Some(why) = &app.offline {
        Line::from(coloured(format!(" {}", why.lines().next().unwrap_or("")), WARN))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line), area);
}

/// A line being typed, in `room` cells with its cursor in view (`back` characters after it):
/// what comes before the cursor, the cell under it marked, what comes after; the cursor's cell
/// from the left; and the first character shown, for a click to count from.
pub(super) fn typed(value: &str, back: usize, room: usize) -> (Vec<Span<'static>>, u16, usize) {
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    let at = n - back.min(n);
    let from = (at + 1).saturating_sub(room.max(1));
    let end = (from + room.max(1)).min(n);
    let before: String = chars[from..at].iter().collect();
    let under = chars.get(at).map_or_else(|| " ".to_string(), |c| c.to_string());
    let after: String = chars.get(at + 1..end).map(|s| s.iter().collect()).unwrap_or_default();
    (vec![Span::raw(before), Span::styled(under, Style::new().fg(Color::Black).bg(Color::Gray)), Span::raw(after)], (at - from) as u16, from)
}

/// Where a line being typed is drawn: the cursor there, and a click placing it.
pub(super) fn typed_at(f: &mut Frame, app: &App, x: u16, y: u16, room: u16, col: u16, from: usize) {
    app.hit(Rect { x, y, width: room, height: 1 }, Click::Input { from });
    f.set_cursor_position((x + col, y));
}

/// Under the `:` line's typing, what it may become, at the foot of the page: tab takes the first.
fn suggest(f: &mut Frame, app: &App, body: Rect) {
    let said = app.suggestions();
    if said.is_empty() || body.height < 6 {
        return;
    }
    let width = said.iter().map(|(l, w)| l.chars().count() + w.chars().count() + 6).max().unwrap_or(0).min(body.width as usize) as u16;
    let height = said.len() as u16;
    let rect = Rect { x: body.x, y: body.y + body.height - height, width, height };
    f.render_widget(ratatui::widgets::Clear, rect);
    let lines: Vec<Line> = said
        .iter()
        .enumerate()
        .map(|(i, (line, what))| {
            let first = i == 0;
            Line::from(vec![Span::styled(format!(" :{line:<16}"), if first { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) }), dim(format!(" {what} ")), dim(if first { "tab " } else { "" })])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(Color::Indexed(236))), rect);
}

/// The page you are on, as the crumbs end.
#[cfg(test)]
pub(crate) fn crumb_last(app: &App) -> String {
    crumbs(app).pop().map(|(s, _)| s).unwrap_or_default()
}

/// Where you are, after the server's name on top: `Flows › refunds › Runs`, each but the
/// last with where a click on it goes.
fn crumbs(app: &App) -> Vec<(String, Option<Click>)> {
    match app.page {
        Page::Flow => {
            let slug = app.detail.as_ref().map(|d| d.slug.clone()).unwrap_or_default();
            let section = super::flow::SECTIONS.iter().find(|(s, _)| *s == app.flow.section).map(|(_, n)| n.to_string()).unwrap_or_default();
            vec![("Flows".into(), Some(Click::Page(Page::Flows))), (slug, Some(Click::Section(Section::Status))), (section, None)]
        }
        Page::Editor => match app.tabs.get(app.tab).filter(|_| !app.home) {
            Some(t) => {
                let mode = if t.runs.is_some() { "Runs" } else if app.typing { "Typing" } else { "Page keys" };
                vec![("Editor".into(), Some(Click::Home)), (t.name.clone(), Some(Click::Tab(app.tab))), (mode.into(), None)]
            }
            None => vec![("Editor".into(), None)],
        },
        Page::Admin => {
            let part = crate::workbench::admin::PARTS.iter().find(|(p, _)| *p == app.admin.part).map(|(_, n)| n.to_string()).unwrap_or_default();
            vec![("Admin".into(), None), (part, None)]
        }
        page => vec![(PAGES.iter().find(|(p, _)| *p == page).map(|(_, n)| n.to_string()).unwrap_or_default(), None)],
    }
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// The shell line, open: who and where, as a shell's prompt says it, what is typed, and what
/// enter does.
fn draw_shell(f: &mut Frame, app: &App, area: Rect, line: &str) {
    let lead = format!(" {} ", super::shell::prompt(area.width as usize / 2));
    let hint = "   enter runs it in this terminal, its prompt then stays for more until esc · ↑ earlier";
    let x = area.x + lead.chars().count() as u16 + 1;
    let room = area.width.saturating_sub(x - area.x + 1);
    let (shown, col, from) = typed(line, app.line_back, room as usize);
    let spans = [vec![Span::styled(lead.clone(), Style::new().fg(Color::Black).bg(GOOD).add_modifier(Modifier::BOLD)), Span::raw(" ")], shown, vec![dim(hint)]].concat();
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(Color::Indexed(235))), area);
    typed_at(f, app, x, area.y, room, col, from);
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
        return draw_shell(f, app, area, line);
    }
    let in_flow_pane = app.flow.right;
    let mut keys: Vec<(&str, &str, KeyCode, KeyModifiers)> = if app.modal.is_some() {
        vec![]
    } else if app.prompt.is_some() {
        vec![("enter", "run", Enter, NONE), ("esc", "cancel", Esc, NONE)]
    } else {
        match app.page {
            Page::Editor if app.tabs.is_empty() => vec![("↑↓", "pick", Down, NONE), ("enter", "open", Enter, NONE), ("n", "new draft", Char('n'), NONE), ("ctrl+p", "find", Char('p'), CTRL), ("q", "quit", Char('q'), NONE)],
            Page::Editor if app.home => vec![("↑↓", "pick", Down, NONE), ("enter", "open", Enter, NONE), ("n", "new draft", Char('n'), NONE), ("esc", "back to the tab", Esc, NONE)],
            Page::Editor if app.tabs[app.tab].runs.is_some() => vec![("↑↓", "run", Down, NONE), ("→", "next step", Right, NONE), ("←", "step back", Left, NONE), (", .", "older · newer", Char(','), NONE), ("tab", "details · errors · timeline", Tab, NONE), ("=", if app.values { "hide values" } else { "show values" }, Char('='), NONE), ("b", show_next(app.show), Char('b'), NONE), ("esc", "back to editing", Esc, NONE)],
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
                Section::Runs => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "step through", Enter, NONE), ("i", "explain", Char('i'), NONE), ("L", "its lines", Char('L'), NONE), ("/", "filter", Char('/'), NONE), ("F", if app.flow.follow { "stop following" } else { "follow" }, Char('F'), NONE), ("o", "export", Char('o'), NONE), ("b", "go back to it", Char('b'), NONE), ("c", "call", Char('c'), NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::Logs => vec![("↑↓", "pick", Down, NONE), ("→", "its line", Right, NONE), ("w", "level", Char('w'), NONE), ("u", "run's lines", Char('u'), NONE), ("F", if app.flow.log_paused { "follow" } else { "pause" }, Char('F'), NONE), ("enter", "step through", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::History => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "go back to it", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::State => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "its history", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::Access => vec![("↑↓", "pick", Down, NONE), ("enter", "change it", Enter, NONE), ("del", "remove a secret", Delete, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
                Section::Graph => vec![("↑↓", "pick", Down, NONE), ("→", "its tabs", Right, NONE), ("enter", "edit it there", Enter, NONE), ("tab", "section", Tab, NONE), ("esc", "flows", Esc, NONE)],
            },
            Page::Overview => vec![("↑↓", "pick", Down, NONE), ("enter", "open", Enter, NONE), ("e", "edit", Char('e'), NONE), ("c", "call", Char('c'), NONE), ("ctrl+p", "find", Char('p'), CTRL), ("q", "quit", Char('q'), NONE)],
            Page::Approvals => vec![("↑↓", "pick", Down, NONE), ("enter", "review and approve", Enter, NONE), ("q", "quit", Char('q'), NONE)],
            Page::Flows => vec![("↑↓", "pick", Down, NONE), ("enter", "open", Enter, NONE), ("e", "edit", Char('e'), NONE), ("c", "call", Char('c'), NONE), ("/", "filter", Char('/'), NONE), ("f", "live · waiting · paused", Char('f'), NONE), ("q", "quit", Char('q'), NONE)],
            Page::Library => vec![("↑↓", "pick", Down, NONE), ("enter", "open a copy", Enter, NONE), ("/", "filter", Char('/'), NONE), ("q", "quit", Char('q'), NONE)],
            Page::Admin if app.admin.right => vec![("↑↓", "scroll", Down, NONE), ("tab", "next part", Tab, NONE), ("esc", "back to the list", Esc, NONE)],
            Page::Admin => {
                use crate::workbench::admin::Part;
                let mut keys = vec![("↑↓", "pick", Down, NONE)];
                keys.extend(match app.admin.part {
                    Part::People => vec![("n", "invite", Char('n'), NONE), ("a", "agent or service", Char('a'), NONE), ("enter", "role", Enter, NONE), ("e", "email", Char('e'), NONE), ("o", "owner", Char('o'), NONE), ("x", "disable", Char('x'), NONE), ("s", "reset link", Char('s'), NONE), ("del", "remove", Delete, NONE)],
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
    let mut keys = bar;
    // At the right: the shell line, closed, in what the keys leave (its path shortened to fit,
    // never below a few letters of it). ctrl+` opens it from anywhere, even while typing. The
    // keys give way from the last (? lists them all) before its prompt is cut below who you are
    // and a dozen letters of the folder.
    let width_of = |keys: &[(&str, &str, KeyCode, KeyModifiers)]| 1 + keys.iter().map(|(k, w, ..)| k.chars().count() + w.chars().count() + 5).sum::<usize>();
    let room = (area.width as usize).saturating_sub(super::shell::who().chars().count() + 22);
    while keys.len() > 1 && width_of(&keys) > room {
        keys.pop();
    }
    let used = width_of(&keys);
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

/// How a deployment is reached: its port, or what starts its runs besides a person (its clock
/// handlers, `every 5s`, and file handlers, `on file inbox`).
pub(super) fn reach(d: &Value) -> String {
    let started = d["triggers"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|t| t.starts_with("every ") || t.starts_with("on file ")).collect::<Vec<_>>();
    if let Some(p) = d["port"].as_u64() {
        format!(":{p}")
    } else if !started.is_empty() {
        started.join(", ")
    } else {
        "by hand".into()
    }
}

/// The colour of a thing done on the server, by its kind: a deploy in the accent, a grant as a
/// flow's history tags it, a pause in warn, a deletion bad, a database the data's colour.
fn activity_colour(action: &str) -> Option<Color> {
    match action {
        "deployed" | "restored" => Some(ACCENT),
        a if a.starts_with("grant") => Some(Color::Magenta),
        "paused" | "stopped" | "asked to restore" => Some(WARN),
        a if a.starts_with("delete") || a.starts_with("removed") || a.starts_with("revoked") || a == "disabled" => Some(BAD),
        a if a.contains("database") || a == "restored state" => Some(DATA),
        "run" => Some(DIM),
        _ => None,
    }
}

/// The version deployed now, as `v3`: the deployment's number, not its unit.
pub(super) fn version(d: &Value) -> String {
    d["version"].as_u64().map(|n| format!("v{n}")).unwrap_or_else(|| "–".into())
}

/// A deployments list's columns, as its rows lay them out: what is shown at `width`.
fn deployment_head(width: usize) -> Line<'static> {
    let mut head = format!("{:<22}{:<16}", "name", "state");
    if width > 124 {
        head.push_str(&format!("{:<10}", "unit"));
    }
    head.push_str(&format!("{:<10}{:<17}", "reach", "runs"));
    if width >= 87 {
        head.push_str(&format!("{:<10}{:<12}", "last 24 h", "last run"));
    }
    if width > 150 {
        head.push_str("deployed");
    }
    Line::from(dim(head))
}

/// A deployment in a row: its name, its state in words ("v2 waits for you" when a version saved
/// since waits for a person), its unit and reach, its runs; wide, its last day and last run.
fn deployment_row(c: &Client, d: &Value, width: usize) -> Line<'static> {
    let slug = d["slug"].as_str().unwrap_or("?");
    let unit = d["unit"].as_str().unwrap_or("");
    let runs = d["trigger_count"].as_u64().unwrap_or(0);
    let failed = d["error_count"].as_u64().unwrap_or(0);
    let deployed_at = d["deployed_at_epoch_seconds"].as_u64().unwrap_or(0);
    let newer = c.saved(slug).is_some_and(|f| f["saved_at_epoch_seconds"].as_u64().unwrap_or(0) > deployed_at);
    let mut state = vec![status(d), Span::raw(" "), Span::raw(version(d))];
    if newer {
        let next = d["version"].as_u64().map(|n| format!("v{}", n + 1)).unwrap_or_else(|| "a version".into());
        state = vec![coloured(format!("{next} waits for you"), WARN)];
    }
    let used: usize = state.iter().map(|s| s.content.chars().count()).sum();
    let mut spans = vec![bold(pad(slug, 22))];
    spans.extend(state);
    spans.push(Span::raw(" ".repeat(16usize.saturating_sub(used))));
    if width > 124 {
        spans.push(dim(format!("{:<10}", &unit[..unit.len().min(8)])));
    }
    spans.push(Span::raw(format!("{:<10}", fit(&reach(d), 9))));
    // Its runs, and how many failed, in one column.
    let said = format!("{runs} run{}", if runs == 1 { "" } else { "s" });
    spans.push(Span::raw(said.clone()));
    let mut used = said.chars().count();
    if failed > 0 {
        let bad = format!(", {failed} failed");
        used += bad.chars().count();
        spans.push(coloured(bad, BAD));
    }
    spans.push(Span::raw(" ".repeat(17usize.saturating_sub(used))));
    if width >= 87 {
        let recent = c.recent.get(slug).map(|(_, r)| r.as_slice()).unwrap_or_default();
        spans.extend(flow::spark(recent, 8).spans);
        spans.push(Span::raw("  "));
        spans.push(match recent.last() {
            Some(r) if crate::workbench::flow::failed(r) => coloured(format!("{:<12}", flow::ago(r["at_ms"].as_u64().unwrap_or(0))), BAD),
            Some(r) => dim(format!("{:<12}", flow::ago(r["at_ms"].as_u64().unwrap_or(0)))),
            None => dim(format!("{:<12}", "never")),
        });
    }
    if width > 150 {
        let by = d["deployed_by"].as_str().map(|b| format!("by {b}, ")).unwrap_or_default();
        spans.push(dim(format!("{by}{}", crate::mcp::when(&d["deployed_at_epoch_seconds"]))));
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

/// The last day's runs across every flow, from each one's latest runs read: how many, how many
/// failed, how long they took; `more` when a flow ran more than was read.
struct Day {
    runs: usize,
    failed: usize,
    p50: Option<f64>,
    p99: Option<f64>,
    more: bool,
}

fn last_day(c: &Client) -> Day {
    let since = flow::now_ms().saturating_sub(24 * 3_600_000);
    let mut times = vec![];
    let (mut count, mut failed, mut more) = (0, 0, false);
    for (_, runs) in c.recent.values() {
        let day: Vec<&Value> = runs.iter().filter(|r| r["at_ms"].as_u64().is_some_and(|a| a >= since)).collect();
        count += day.len();
        more |= day.len() as u64 >= crate::workbench::client::RECENT;
        failed += day.iter().filter(|r| crate::workbench::flow::failed(r)).count();
        times.extend(day.iter().filter_map(|r| r["duration_ms"].as_f64()));
    }
    times.sort_by(f64::total_cmp);
    let at = |p: f64| (!times.is_empty()).then(|| times[((times.len() - 1) as f64 * p).round() as usize]);
    Day { runs: count, failed, p50: at(0.5), p99: at(0.99), more }
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
    let asked: usize = waiting.iter().take(6).map(|(slug, w)| if w.starts_with("to start") || w.starts_with("to free") { 0 } else { app.asks(slug).len().min(3) }).sum();
    let wait_height = (waiting.len().clamp(1, 6) + asked + warned.len().min(3) * 2 + 2) as u16;
    let [said, tiles, rest] = Layout::vertical([Constraint::Length(1), Constraint::Length(tiles_height), Constraint::Min(3)]).areas(area);
    // The page in a line: who, what waits, what runs, what failed.
    let who = c.me.clone().map(|m| format!("{m}: ")).unwrap_or_default();
    let mut head = vec![Span::raw(" "), bold(who)];
    head.push(if waiting.is_empty() { coloured("nothing waits for you", GOOD) } else { coloured(format!("{} wait{} for you", waiting.len(), if waiting.len() == 1 { "s" } else { "" }), WARN) });
    head.push(dim(format!("  ·  {running} of {} running", c.deployments.len())));
    head.push(if failed > 0 { coloured(format!("  ·  {failed} failed of {runs} runs"), BAD) } else { dim(format!("  ·  {runs} runs, none failed")) });
    f.render_widget(Paragraph::new(Line::from(head)), said);
    if tiles_height > 0 {
        let parts = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(tiles);
        tile(f, parts[0], "Deployed", &c.deployments.len().to_string(), &format!("{running} running"), ACCENT);
        tile(f, parts[1], "Waiting for you", &waiting.len().to_string(), if waiting.is_empty() { "nothing to approve" } else { "2 opens them" }, if waiting.is_empty() { GOOD } else { WARN });
        let day = last_day(c);
        let at_least = if day.more { "≥" } else { "" };
        let times = match (day.p50, day.p99) {
            (Some(a), Some(b)) => format!("p50 {} · p99 {}", crate::workbench::flow::took(a), crate::workbench::flow::took(b)),
            _ => format!("{runs} on record"),
        };
        tile(f, parts[2], "Runs, last 24 h", &format!("{at_least}{}", day.runs), &times, Color::White);
        let rate = if day.runs > 0 { format!("{:.1}% of them · {failed} on record", day.failed as f64 * 100.0 / day.runs as f64) } else { format!("{failed} on record") };
        tile(f, parts[3], "Failed, last 24 h", &day.failed.to_string(), &rate, if day.failed > 0 { BAD } else { GOOD });
    }
    // Lately beside the deployments only when their rows keep room for their last day.
    let wide = rest.width >= 160 && !c.activity.is_empty();
    let [left, right] = Layout::horizontal([Constraint::Min(40), Constraint::Length(if wide { 52 } else { 0 })]).areas(rest);
    let [deps, waits] = Layout::vertical([Constraint::Min(4), Constraint::Length(wait_height)]).areas(left);

    let block = panel(&format!("Deployed on {}", c.server()), true);
    let inner = block.inner(deps);
    f.render_widget(block, deps);
    keys_here(app, deps);
    if c.deployments.is_empty() {
        f.render_widget(Paragraph::new(dim("Nothing deployed yet. Open a sample (6), or a file with :open <file>, and deploy it with ctrl+d.")).wrap(Wrap { trim: false }), inner);
    } else {
        let [head, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        f.render_widget(Paragraph::new(deployment_head(head.width as usize)), Rect { x: head.x + 1, width: head.width.saturating_sub(1), ..head });
        list(f, app, rows, List::Page(Page::Overview), c.deployments.len(), Some(app.sel[0]), true, |i, w| deployment_row(c, &c.deployments[i], w));
    }

    let mut lines: Vec<Line> = warned.iter().take(3).flat_map(|(says, todo)| [Line::from(vec![coloured("! ", BAD), Span::raw(says.clone())]), Line::from(dim(format!("  {todo}")))]).collect();
    if waiting.is_empty() {
        lines.push(Line::from(dim("Nothing waits for a person.")));
    } else {
        // Each wait, then what its version asks that the flow may not do now.
        for (slug, w) in waiting.iter().take(6) {
            lines.push(Line::from(vec![coloured("● ", WARN), bold(format!("{slug}  ")), Span::raw(w.clone())]));
            let asks = if w.starts_with("to start") || w.starts_with("to free") { vec![] } else { app.asks(slug) };
            let more = asks.len().saturating_sub(2);
            for ask in asks.iter().take(2) {
                lines.push(Line::from(vec![Span::raw("    "), coloured("+ ", WARN), dim(ask.clone())]));
            }
            if more > 0 {
                lines.push(Line::from(dim(format!("    and {more} more: Approvals shows them"))));
            }
        }
    }
    let room = waits.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = lines.into_iter().map(|l| fit_line(l, room)).collect();
    f.render_widget(Paragraph::new(lines).block(panel(&format!("Waiting for you ({})", waiting.len()), false)), waits);
    app.hit(waits, Click::Page(Page::Approvals));

    if wide {
        // Each line in the colour of what was done, as a flow's history tags it.
        let lines: Vec<Line> = crate::flowscmd::activity_lines(&json!({ "entries": c.activity }))
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                let l = fit(&l, right.width.saturating_sub(2) as usize);
                match c.activity.get(i).and_then(|e| e["action"].as_str()).map(activity_colour) {
                    Some(Some(colour)) => Line::from(coloured(l, colour)),
                    _ => Line::from(l),
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines).block(panel("Lately", false)), right);
    }
}

/// What waits for a person on the left; on the right, the one picked, reviewed as its deploy
/// would be: enter approves it there.
fn draw_approvals(f: &mut Frame, app: &mut App, area: Rect) {
    if offline(f, app, area, "Approvals") {
        return;
    }
    app.sync_approval();
    let waiting = app.waiting();
    let review_keys = app.approval.as_ref().is_some_and(|a| a.keys);
    let wide = area.width >= 110;
    let [left, right] = if wide { Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(area) } else { Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area) };
    let block = panel(&format!("Waiting for you ({})", waiting.len()), !review_keys);
    if !review_keys {
        keys_here(app, left);
    }
    let inner = block.inner(left);
    f.render_widget(block, left);
    if waiting.is_empty() {
        f.render_widget(Paragraph::new(vec![Line::from(coloured(" Nothing waits for you.", GOOD)), Line::default(), Line::from(dim(" A flow saved for review, or one paused, shows here."))]), inner);
        return;
    }
    let sel = app.sel[1].min(waiting.len() - 1);
    list(f, app, inner, List::Page(Page::Approvals), waiting.len(), Some(sel), !review_keys, |i, w| {
        let (slug, wait) = &waiting[i];
        let (mark, colour) = if wait.starts_with("to start it") { ("◷ ", WARN) } else if wait.starts_with("to free") { ("! ", BAD) } else { ("● ", WARN) };
        Line::from(vec![coloured(mark, colour), bold(pad(slug, 18)), dim(fit(wait, w.saturating_sub(20)))])
    });

    // Taken out while drawn, so its review can keep its scroll; put back after.
    let Some(mut a) = app.approval.take() else { return };
    let title = format!("{}  ·  {} of {}", a.of.0, sel + 1, waiting.len());
    let block = panel(&title, review_keys);
    let inner = block.inner(right);
    f.render_widget(block, right);
    match &mut a.what {
        crate::workbench::app::Pending::Deploy(s) => {
            let may_height = (s.may.len().min(8) + 2) as u16;
            let [said, shown, may] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(may_height)]).areas(inner);
            f.render_widget(Paragraph::new(dim(fit(&a.of.1, said.width as usize))), said);
            modal::review_body(f, app, shown, &mut s.review, review_keys);
            // If approved: what it may do while it runs, what is new marked +.
            let lines: Vec<Line> = s.may.iter().zip(&s.new).take(8).map(|(m, new)| if *new { Line::from(vec![coloured("+ ", WARN), Span::raw(m.clone())]) } else { Line::from(vec![dim("· "), dim(m.clone())]) }).collect();
            f.render_widget(Paragraph::new(lines).block(panel("If approved, while it runs it may", false)), may);
        }
        crate::workbench::app::Pending::Start => {
            f.render_widget(Paragraph::new(vec![Line::from(format!("\"{}\" is {}.", a.of.0, app.status_of(&a.of.0))), Line::default(), Line::from(dim("Enter runs it again, as you."))]).wrap(Wrap { trim: false }), inner);
        }
        crate::workbench::app::Pending::Said(why) => f.render_widget(Paragraph::new(why.clone()).wrap(Wrap { trim: false }), inner),
        crate::workbench::app::Pending::Failed(why) => f.render_widget(Paragraph::new(vec![Line::from(coloured("It can't be reviewed here:", BAD)), Line::from(why.clone())]).wrap(Wrap { trim: false }), inner),
    }
    let go = match &a.what {
        crate::workbench::app::Pending::Deploy(s) if s.staged.needs.is_empty() => "deploy",
        crate::workbench::app::Pending::Deploy(_) => "approve and deploy",
        crate::workbench::app::Pending::Start => "run it",
        _ => "",
    };
    app.approval = Some(a);
    let mut keys: Vec<(&str, &str, KeyCode, KeyModifiers)> = vec![];
    if !go.is_empty() {
        keys.push(("enter", go, KeyCode::Enter, KeyModifiers::NONE));
    }
    keys.extend([("]", "next", KeyCode::Char(']'), KeyModifiers::NONE), ("e", "open in the editor", KeyCode::Char('e'), KeyModifiers::NONE)]);
    keys.push(if review_keys { ("esc", "back to the list", KeyCode::Esc, KeyModifiers::NONE) } else { ("→", "read it", KeyCode::Right, KeyModifiers::NONE) });
    let bar = Rect { x: right.x + 2, y: right.y + right.height - 1, width: right.width.saturating_sub(4), height: 1 };
    buttons(f, app, bar, &keys);
}

fn draw_flows(f: &mut Frame, app: &App, area: Rect) {
    if offline(f, app, area, "Flows") {
        return;
    }
    let c = app.client.as_ref().unwrap();
    let names = app.flow_names();
    let all = app.all_flow_names().len();
    let block = panel(&listed("Flows", names.len(), all, app.filter_of(Page::Flows)), true);
    keys_here(app, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [chips_at, head, rows] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    // Which flows it lists, a chip each (f goes round them).
    let mut x = chips_at.x + 1;
    for (s, name) in crate::workbench::app::SHOWN {
        let label = format!(" {name} ");
        let w = label.chars().count() as u16;
        if x + w > chips_at.x + chips_at.width {
            break;
        }
        let style = if s == app.shown { Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray).bg(Color::Indexed(236)) };
        let rect = Rect { x, width: w, ..chips_at };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        app.hit(rect, Click::Shown(s));
        x += w + 1;
    }
    f.render_widget(Paragraph::new(dim(" f  next   /  filter")), Rect { x, width: chips_at.width.saturating_sub(x - chips_at.x), ..chips_at });
    f.render_widget(Paragraph::new(Line::from(vec![dim(format!("  {:<22}{:<11}{:<5}{:<10}{:<10}", "name", "state", "ver", "unit", "reach")), dim("saved")])), head);
    if names.is_empty() {
        f.render_widget(Paragraph::new(dim(if all == 0 { " No flows on this server yet." } else { " None of them, as picked: f, esc or another filter." })), rows);
        return;
    }
    list(f, app, rows, List::Page(Page::Flows), names.len(), Some(app.sel[2]), true, |i, w| {
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
                spans.extend([st, Span::raw(" ".repeat(n)), Span::raw(format!("{:<5}", version(d))), dim(format!("{:<10}", &unit[..unit.len().min(8)])), Span::raw(format!("{:<10}", reach(d)))]);
            }
            None => spans.push(coloured(format!("{:<36}", "not deployed"), DIM)),
        }
        spans.push(dim(saved.unwrap_or_default()));
        fit_line(Line::from(spans), w)
    });
}

/// A list's title: how many rows, of how many when a filter narrows it, and the filter.
fn listed(what: &str, n: usize, of: usize, filter: &str) -> String {
    match (filter.is_empty(), n == of) {
        (true, true) => format!("{what} ({n})"),
        (true, false) => format!("{what} ({n} of {of})"),
        (false, _) => format!("{what} ({n} of {of})  /{filter}"),
    }
}

fn draw_library(f: &mut Frame, app: &App, area: Rect) {
    let all = app.library();
    let sel = app.sel[5].min(all.len().saturating_sub(1));
    let [left, about] = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
    let block = panel(&listed("Samples", all.len(), samples::samples().len(), app.filter_of(Page::Library)), true);
    keys_here(app, left);
    let inner = block.inner(left);
    f.render_widget(block, left);
    list(f, app, inner, List::Page(Page::Library), all.len(), Some(sel), true, |i, w| Line::from(vec![bold(pad(all[i].name, 20)), dim(fit(all[i].title, w.saturating_sub(20)))]));
    if let Some(s) = all.get(sel) {
        let mut lines = vec![Line::from(bold(s.title.to_string())), Line::default(), Line::from(s.description.to_string()), Line::default()];
        // What it shows, a chip each.
        let mut chips = vec![];
        for tag in s.shows {
            chips.push(Span::styled(format!(" {tag} "), Style::new().fg(Color::Black).bg(Color::Gray)));
            chips.push(Span::raw(" "));
        }
        lines.push(Line::from(chips));
        lines.push(Line::default());
        lines.push(Line::from(dim("From a shell")));
        lines.push(Line::from(coloured(format!("  holon new {}.holon.ts --sample {}", s.name, s.name), ACCENT)));
        lines.push(Line::default());
        lines.push(Line::from(dim("Try it")));
        lines.extend(s.try_it.lines().map(|l| Line::from(format!("  {l}"))));
        lines.push(Line::default());
        lines.push(Line::from(dim("Enter (or a second click) opens a copy in a tab: it is yours to edit, save and deploy.")));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(s.name, false)), about);
    }
}
