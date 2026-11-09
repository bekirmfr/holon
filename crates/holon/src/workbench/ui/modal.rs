//! The dialogs: approving a deploy (and what it changes), going back to a point and its dry run,
//! the call form, the palette, a line typed in, a yes or no, the keys. Each has its buttons at
//! the bottom, to press or click.

use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::{bold, coloured, dim, fit, list, pad, panel, scrollbar, tone, ACCENT, GOOD, WARN};
use crate::workbench::app::{App, Click, List, Modal, Said, Tone};
use crate::workbench::flow::restore_options;
use crate::workbench::palette;
use crate::workbench::review::{Code, Review, Row};

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
    ("ctrl+t", "a new tab with a draft in it (ctrl+n too, the tab bar's + too); its first save asks for a file"),
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
    ("", "graph: edit the statement picked; a (or its button on the graph) shows the statements, or every node again"),
    ("b", "go back to the run or version picked"),
    ("i  o  F  /", "runs: explain the one picked; export them to <slug>-runs.json for holon verify; follow the newest; filter"),
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
    ("alt+g", "side by side: the graph takes the keys (a click on it too): ↑ ↓ pick, marked in the text;"),
    ("", "→ its detail, enter edits it there, a every node or the statements, esc back to the text"),
    ("ctrl+r", "a run over its text: what it played lit, the rest dimmed, each value under its line;"),
    ("", "below, the runs (↑ ↓) and the one picked: tab goes round Details, Errors and Timeline;"),
    ("", "← → its timeline's steps, marked in the code and the graph (a click on the line under the code too);"),
    ("", ", . the run before or after; = hides or shows the values; b the layout"),
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
    ("A deploy to approve, a dry run", ""),
    ("↑ ↓", "a line at a time (pgup pgdn a page, home end)"),
    ("n  N", "the next hunk or fold, the one before (tab, shift+tab too)"),
    ("space  → ←", "open or close the hunk the line is in (a click on its head too); a fold opens"),
    ("w  d", "the whole of it, every hunk open; back to the diff"),
    ("enter  esc", "approve it (deploy, restore); cancel (x too)"),
    ("s", "a deploy: save it on the server, not deployed, for whoever may approve it (Approvals)"),
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
    super::buttons(f, app, bar, buttons);
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

/// The grounds a review's rows are tinted with: put in or new, taken out or gone (or not ready),
/// and to approve or changed.
const ADDED_GROUND: Color = Color::Rgb(20, 46, 34);
const REMOVED_GROUND: Color = Color::Rgb(54, 22, 26);
const ASKED_GROUND: Color = Color::Rgb(50, 42, 18);
/// A line taken out: its text quieter than the rest.
const REMOVED_TEXT: Color = Color::Rgb(150, 112, 112);

/// A review (a deploy to approve, a restore's dry run): its hunks, each opening and closing, each
/// a diff drawn alike; the cursor on a row, marked down the left. `go` is what enter does (none
/// when it can't be done); `foot` stays under it, outside what scrolls.
#[allow(clippy::too_many_arguments)]
fn reviewed(f: &mut Frame, app: &mut App, area: Rect, review: &mut Review, title: &str, go: &str, other: Option<(&'static str, &'static str)>, foot: Option<(&str, Color)>) {
    let rows = review.rows();
    let mut buttons = vec![];
    if !go.is_empty() {
        buttons.push(("enter", go, KeyCode::Enter, NONE));
    }
    // What else it may do than enter's: save it for another to approve.
    if let Some((key, what)) = other {
        buttons.push((key, what, KeyCode::Char(key.chars().next().unwrap_or(' ')), NONE));
    }
    // With two ways out, the arrows go unsaid: the buttons keep to one line.
    if other.is_none() {
        buttons.push(("↑ ↓", "lines", KeyCode::Down, NONE));
    }
    buttons.extend([("n", "next hunk", KeyCode::Char('n'), NONE), ("space", "open/close", KeyCode::Char(' '), NONE)]);
    buttons.push(if review.whole { ("d", "diff", KeyCode::Char('d'), NONE) } else { ("w", "whole", KeyCode::Char('w'), NONE) });
    buttons.push(("esc", "cancel", KeyCode::Esc, NONE));
    let rect = centred(area, 120, rows.len() as u16 + 6 + u16::from(foot.is_some()));
    let body = frame(f, app, rect, title, &buttons);
    let [shown, under] = Layout::vertical([Constraint::Min(1), Constraint::Length(u16::from(foot.is_some()))]).areas(body);
    if let Some((said, colour)) = foot {
        f.render_widget(Paragraph::new(coloured(said, colour)), under);
    }
    review_body(f, app, shown, review, true);
}

/// A review's rows in `shown`, scrolled to keep its cursor in sight; the cursor marked when the
/// review has the keys.
pub(super) fn review_body(f: &mut Frame, app: &App, shown: Rect, review: &mut Review, focused: bool) {
    let rows = review.rows();
    review.height = shown.height as usize;
    review.scroll = review.scroll.min(rows.len().saturating_sub(shown.height as usize));
    let width = shown.width.saturating_sub(2) as usize;
    let cursor = review.cursor(&rows);
    app.hit(shown, crate::workbench::app::Click::Scroll(List::Dialog));
    let mut lines = vec![];
    for (i, row) in rows.iter().enumerate().skip(review.scroll).take(shown.height as usize) {
        let (mut line, ground) = review_row(review, *row, width);
        let here = focused && i == cursor && *row != Row::Blank;
        line.spans.insert(0, if here { coloured("▌", ACCENT) } else { Span::raw(" ") });
        if let Some(g) = ground.or(here.then_some(super::PICKED)) {
            line = line.style(Style::new().bg(g));
        }
        let y = shown.y + (i - review.scroll) as u16;
        app.hit(Rect { y, height: 1, ..shown }, crate::workbench::app::Click::Row(List::Review, i));
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), shown);
    scrollbar(f, shown, rows.len(), review.scroll);
}

/// A row as drawn, and the ground it is tinted with.
fn review_row(review: &Review, row: Row, width: usize) -> (Line<'static>, Option<Color>) {
    match row {
        Row::Top(i) => (said_line(&review.top[i], width), None),
        Row::Foot(i) => (said_line(&review.foot[i], width), None),
        Row::Blank => (Line::default(), None),
        Row::Head(h) => {
            let hunk = &review.hunks[h];
            let open = hunk.open || review.whole;
            let mut spans = vec![coloured(if open { "▾ " } else { "▸ " }, ACCENT), bold(hunk.head.clone())];
            for (k, (t, s)) in hunk.sum.iter().enumerate() {
                spans.push(dim(if k == 0 { "   " } else { " · " }));
                spans.push(Span::styled(s.clone(), tone(*t)));
            }
            (Line::from(spans), None)
        }
        Row::Fold(_, _, n) => (Line::from(dim(format!("{:>6} ⋯ {n} unchanged line{} (space shows them)", "", if n == 1 { "" } else { "s" }))), None),
        Row::Code(h, i) => code_line(&review.hunks[h].rows[i], width),
    }
}

fn said_line((t, s): &Said, width: usize) -> Line<'static> {
    Line::from(Span::styled(fit(s, width), tone(*t)))
}

/// A row of a review's diff (a line of code, a thing it touches, a value it puts back): its
/// number, its mark, its text coloured, the names a permission comes from marked, its note; and
/// the ground that says what the change does to it.
fn code_line(c: &Code, width: usize) -> (Line<'static>, Option<Color>) {
    let (sign, ground) = match c.kind {
        '+' => (GOOD, Some(ADDED_GROUND)),
        'a' | '~' => (WARN, Some(ASKED_GROUND)),
        '-' | '!' => (super::BAD, Some(REMOVED_GROUND)),
        _ => (super::DIM, None),
    };
    let chars: Vec<char> = c.text.chars().collect();
    let room = width.saturating_sub(7);
    let plain = match c.kind {
        'i' => Style::new().fg(super::DIM),
        'w' => Style::new().fg(WARN),
        '-' => Style::new().fg(REMOVED_TEXT),
        _ => Style::new(),
    };
    let mut styles: Vec<Style> = vec![plain; chars.len()];
    if c.kind != '-' {
        for &(from, to, class) in &c.colours {
            for s in styles.iter_mut().take(to).skip(from) {
                *s = super::editor::colour(class);
            }
        }
    }
    for &(from, to) in &c.marked {
        for s in styles.iter_mut().take(to).skip(from) {
            *s = s.fg(WARN).add_modifier(Modifier::UNDERLINED);
        }
    }
    let mut text: Vec<Span<'static>> = vec![];
    for (ch, style) in chars.iter().zip(styles).take(room) {
        match text.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(*ch),
            _ => text.push(Span::styled(ch.to_string(), style)),
        }
    }
    let number = c.number.map(|n| format!("{n:>4} ")).unwrap_or_else(|| " ".repeat(5));
    let mut spans = vec![dim(number), coloured(c.sign(), sign)];
    spans.extend(text);
    if chars.len() > room {
        spans.push(dim("…"));
    } else if !c.note.is_empty() {
        let left = room - chars.len();
        if left > 6 {
            spans.push(Span::styled(fit(&format!("  {}", c.note), left), Style::new().fg(if c.kind == '!' { super::BAD } else { super::DIM })));
        }
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    (Line::from(spans), ground)
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
    // Taken out while drawn, so the dialog can keep its scroll in range; put back after.
    let Some(mut modal) = app.modal.take() else { return };
    match &mut modal {
        Modal::Approve { slug, staged, review, .. } => {
            let title = if staged.needs.is_empty() { format!("Deploy {slug}?") } else { format!("Approve and deploy {slug}?") };
            let go = if staged.needs.is_empty() { "deploy" } else { "approve and deploy" };
            reviewed(f, app, area, review, &title, go, Some(("s", "save, don't deploy")), None);
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
            let room = input.width.saturating_sub(2);
            let (shown, col, from) = super::typed(query, app.line_back, room as usize);
            let hint = dim(if query.is_empty() { "  a flow, a page, a sample, deploy, call…" } else { "" });
            f.render_widget(Paragraph::new(Line::from([vec![coloured("› ", ACCENT)], shown, vec![hint]].concat())), input);
            super::typed_at(f, app, input.x + 2, input.y, room, col, from);
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
        Modal::SecretSource(c) => {
            use crate::workbench::flow::Source;
            let choices = c.choices();
            let suggested = c.suggested();
            let mut about = vec![Line::from(match &c.current {
                Some(from) => vec![dim("Now: "), Span::raw(format!("read from {from}")), dim(". Setting it makes a new version; the old one stays, to roll back to.")],
                None => vec![coloured("Not set: ", WARN), dim("a run that reads it gets nothing until it is.")],
            })];
            // Where the names come from, and how one gets there: a line in the file, read within a minute.
            let place = c.dotenv.as_ref().map(|f| format!("the server's environment or {f}")).unwrap_or_else(|| "the server's environment (or a file it is started with --dotenv FILE)".into());
            if c.env.is_empty() {
                about.push(Line::from(dim(format!("It reads no {}… name now. A line {suggested}=… in {place} shows here within a minute.", crate::workbench::flow::ENV))));
            } else if !c.env.contains(&suggested) {
                about.push(Line::from(dim(format!("{suggested} isn't among the names it reads: a line {suggested}=… in {place} shows here within a minute."))));
            }
            let height = (choices.len() as u16).min(12) + about.len() as u16 * 2 + 6;
            let rect = centred(area, 84, height);
            let body = frame(f, app, rect, &format!("Where {} gets {}", c.slug, c.name), &[("enter", "choose", KeyCode::Enter, NONE), ("esc", "cancel", KeyCode::Esc, NONE)]);
            let [top, rows] = Layout::vertical([Constraint::Length(about.len() as u16 * 2), Constraint::Min(1)]).areas(body);
            f.render_widget(Paragraph::new(about).wrap(Wrap { trim: false }), top);
            let now = c.current.clone().unwrap_or_default();
            list(f, app, rows, List::SecretSource, choices.len(), Some(c.sel), true, |i, w| {
                let (kind, what, note) = match &choices[i] {
                    Source::Value => ("value", "typed here, sealed on the server".to_string(), if now == "local" { "now" } else { "" }),
                    Source::Env(v) => ("env", v.clone(), if now == format!("env:{v}") { "now" } else if *v == suggested { "its name" } else { "" }),
                    Source::OtherEnv => ("env", "another name…".to_string(), ""),
                    Source::Vault => ("vault", "<mount>/<path>#<field>…".to_string(), if now.starts_with("vault:") { "now" } else { "" }),
                };
                let note = if note.is_empty() { String::new() } else { format!("  ← {note}") };
                Line::from(vec![coloured(format!("{kind:<7}"), ACCENT), Span::raw(fit(&what, w.saturating_sub(8 + note.chars().count()))), dim(note)])
            });
        }
        Modal::DryRun { point, review, refused, needs, .. } => {
            let go = if *refused { None } else if needs.is_empty() { Some("restore it") } else { Some("approve and restore it") };
            let foot = if *refused { ("It can't be done now.", super::BAD) } else { ("Nothing has changed yet.", WARN) };
            reviewed(f, app, area, review, &format!("Dry run: going back to {}", point.name), go.unwrap_or(""), None, Some(foot));
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
                let room = parts[i].width.saturating_sub(16);
                let name = Span::styled(format!("{:<13}", form.field_name(i)), if focused { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(super::DIM) });
                let field = Style::new().bg(Color::Indexed(if focused { 238 } else { 235 }));
                if focused {
                    let (shown, col, from) = super::typed(value, app.line_back, room as usize);
                    let shown = shown.into_iter().map(|s| { let style = field.patch(s.style); s.style(style) });
                    f.render_widget(Paragraph::new(Line::from([vec![name, Span::styled(" ", field)], shown.collect(), vec![Span::styled(" ", field)]].concat())), parts[i]);
                    super::typed_at(f, app, parts[i].x + 14, parts[i].y, room, col, from);
                } else {
                    f.render_widget(Paragraph::new(Line::from(vec![name, Span::styled(format!(" {} ", fit(value, room as usize)), field)])), parts[i]);
                }
            }
            let answer = parts[n + 1];
            if form.answer.is_empty() {
                f.render_widget(Paragraph::new(dim("Its answer shows here, and the run on the flow's page.")), answer);
            } else {
                f.render_widget(Paragraph::new(said(&form.answer, answer.width as usize)).block(panel("Answer", false)), answer);
            }
        }
        Modal::Input { title, lines, value, at, masked, .. } => {
            let rect = centred(area, 72, lines.len() as u16 + 8);
            let body = frame(f, app, rect, title, &[("enter", "set it", KeyCode::Enter, NONE), ("esc", "cancel", KeyCode::Esc, NONE)]);
            let [about, field] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(body);
            f.render_widget(Paragraph::new(lines.iter().map(|l| Line::from(l.clone())).collect::<Vec<_>>()).wrap(Wrap { trim: false }), about);
            // As much of it as fits, from where the cursor stays in view.
            let n = value.chars().count();
            let at = (*at).min(n);
            let room = field.width.saturating_sub(4).max(1) as usize;
            let from = at.saturating_sub(room);
            let shown: String = if *masked { "•".repeat(n) } else { value.clone() }.chars().skip(from).take(room).collect();
            f.render_widget(Paragraph::new(Line::from(vec![coloured("› ", ACCENT), Span::styled(format!("{shown} "), Style::new().bg(Color::Indexed(238)))])), field);
            app.hit(Rect { x: field.x + 2, width: field.width.saturating_sub(2), ..field }, Click::Input { from });
            f.set_cursor_position((field.x + 2 + (at - from) as u16, field.y));
        }
    }
    app.modal = Some(modal);
}
