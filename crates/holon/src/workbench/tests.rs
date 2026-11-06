//! `holon editor` driven by its keys against the real router, in-process, and drawn to a test
//! screen: what a person sees and does there.

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;

use super::app::{App, Click, List, Modal, Page, RunPane, Runs, Show, Tone};
use super::client::{Client, Moment};
use super::flow::{Row, Section};
use super::ui;
use crate::mcp::{Api, Body};
use crate::tests::{body_bytes, req, test_state};
use crate::{route, AppState};

/// The router in-process, as the editor's server; signed in, on a server with accounts.
struct InProcess(&'static AppState, Option<String>);

impl Api for InProcess {
    fn server(&self) -> String {
        "the test server".into()
    }
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
        let bytes = match body {
            None => vec![],
            Some(Body::Json(v)) => serde_json::to_vec(&v).unwrap(),
            Some(Body::Bytes(b)) => b,
        };
        let mut request = req(method, path, bytes.clone());
        if let Some(token) = &self.1 {
            let mut headers: Vec<(String, String)> = request.headers().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            headers.push(("Authorization".into(), format!("Bearer {token}")));
            request = rouille::Request::fake_http(method, path, headers, bytes);
        }
        let response = route(self.0, &request);
        Ok((response.status_code, body_bytes(response)))
    }
}

fn app() -> (App, &'static AppState) {
    let state: &'static AppState = Box::leak(Box::new(test_state()));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, None))));
    (app, state)
}

/// The editor on a server with accounts, signed in as its first admin, ada.
fn admin_app() -> App {
    let mut state = test_state();
    let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let (token, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    state.access = crate::access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
    let state: &'static AppState = Box::leak(Box::new(state));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, Some(token)))));
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) });
    }
}

/// The screen, as lines of text.
fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
}

const BEATS: &str = "flow beats\nevery 5s\nstate beats\n\nstate.beats = (state.beats ?? 0) + 1\n";
/// A flow run by hand.
const BUMP: &str = "flow bump\nstate n\n\naction bump (input) {\n  n = (state.n ?? 0) + 1\n  state.n = n\n  respond n\n}\n";

/// Open a copy of `text` in a tab, as a sample opens.
fn tab_with(app: &mut App, text: &str) {
    app.tabs.push(super::app::Tab::new("new", text, None, None));
    app.tab = app.tabs.len() - 1;
    app.page = Page::Editor;
    app.typing = true;
}

#[test]
fn deploying_shows_what_it_may_do_and_deploys_on_enter() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    ctrl(&mut app, 'd');
    let Some(Modal::Approve { slug, plan, .. }) = &app.modal else { panic!("no dialog: {:?}", app.note.as_ref().map(|n| &n.text)) };
    assert_eq!(slug, "beats");
    assert!(!plan.may.is_empty());
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("Deploying \"beats\""), "{shown}");
    assert!(shown.contains("It may"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let note = app.note.as_ref().unwrap();
    assert!(!note.error && note.text.starts_with("deployed \"beats\""), "{}", note.text);
    assert_eq!(app.tabs[0].slug.as_deref(), Some("beats"));
    assert!(!app.tabs[0].dirty(), "deployed: its text is what the server keeps");
    assert!(app.client.as_ref().unwrap().deployment("beats").is_some());

    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('1'));
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("beats") && shown.contains("running") && shown.contains("every 5s"), "{shown}");
}

#[test]
fn a_flow_renamed_in_its_text_deploys_under_its_new_name() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.tabs[0].slug.as_deref(), Some("beats"));
    let renamed = BEATS.replace("flow beats", "flow pulse");
    app.tabs[0].buf.replace(&renamed);
    app.tabs[0].check = super::app::Check::of(&renamed);
    assert_eq!(app.tabs[0].target().as_deref(), Some("pulse"), "its runs, its grant: the new name's");
    ctrl(&mut app, 'd');
    let Some(Modal::Approve { slug, .. }) = &app.modal else { panic!("{:?}", app.note.as_ref().map(|n| &n.text)) };
    assert_eq!(slug, "pulse");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("deploys as a new flow") && shown.contains("\"beats\", which this tab deployed before, stays"), "{shown}");
    press(&mut app, KeyCode::Enter);
    let c = app.client.as_ref().unwrap();
    assert!(c.deployment("pulse").is_some() && c.deployment("beats").is_some());
}

#[test]
fn escape_cancels_a_deploy() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    ctrl(&mut app, 'd');
    assert!(matches!(app.modal, Some(Modal::Approve { .. })));
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert!(app.client.as_ref().unwrap().deployment("beats").is_none());
}

#[test]
fn typing_edits_the_text_and_it_is_checked_once_typing_settles() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    app.tabs[0].buf.go(4, 99);
    typed(&mut app, "\nx = 1 +");
    assert!(app.tabs[0].dirty());
    assert!(app.tabs[0].check.mistakes.is_empty(), "not yet checked");
    app.tabs[0].edited = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    app.tick();
    assert!(!app.tabs[0].check.mistakes.is_empty(), "a statement cut short is a mistake");
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("Problems (1)"), "{shown}");

    // Esc leaves the text: then a digit is a page, not a character.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('6'));
    assert_eq!(app.page, Page::Library);
}

#[test]
fn a_saved_flow_waits_for_a_person_who_approves_it_from_the_approvals_page() {
    let (mut app, _) = app();
    let plan = holon_author::agent::plan(BEATS).unwrap();
    app.client.as_ref().unwrap().save("beats", &plan, BEATS).unwrap();
    app.refresh();
    let waiting = app.waiting();
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].1.starts_with("to deploy it for the first time"), "{waiting:?}");
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("Waiting for you (1)"), "the overview counts what waits: {shown}");
    press(&mut app, KeyCode::Char('2'));
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("beats") && shown.contains("first time"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Editor);
    assert!(matches!(app.modal, Some(Modal::Approve { .. })), "opens it and asks to approve it");
    press(&mut app, KeyCode::Enter);
    assert!(app.waiting().is_empty(), "{:?}", app.waiting());
}

/// Deploy BUMP from a tab, and run its action `times` times.
fn bumped(times: usize) -> (App, &'static AppState) {
    let (mut app, state) = app();
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Enter);
    for _ in 0..times {
        let ran = route(state, &req("POST", "/trigger/bump/bump", b"{}".to_vec()));
        let status = ran.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(ran)));
    }
    app.refresh();
    (app, state)
}

fn pane(app: &App) -> Vec<Row> {
    app.flow.pane.as_ref().map(|(_, rows)| rows.clone()).unwrap_or_default()
}

#[test]
fn a_flows_page_is_a_dashboard_then_each_record_beside_its_tabs() {
    let (mut app, _) = bumped(2);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Flow);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Last 24 h") && shown.contains("Typical run") && shown.contains("It may") && shown.contains("Versions"), "{shown}");
    assert!(!shown.contains("state.n = n"), "the dashboard doesn't show its text: {shown}");

    // Runs: the list on the left, the picked one's tabs on the right.
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.flow.section, Section::Runs);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Runs, newest first (2)") && shown.contains(" Detail ") && shown.contains(" Timeline "), "{shown}");
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Run" && v == "#1")), "{:?}", pane(&app));
    press(&mut app, KeyCode::Down);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Run" && v == "#0")), "the pane follows the pick");
    press(&mut app, KeyCode::Right);
    assert!(app.flow.right);
    press(&mut app, KeyCode::Right);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Code { notes, .. } if notes.iter().any(|n| n.contains('⇒')))), "steps: {:?}", pane(&app));
    press(&mut app, KeyCode::Right);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Bar { .. })), "the timeline: {:?}", pane(&app));
    press(&mut app, KeyCode::Right);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(_, t) if t == "It didn't fail.")));
    for (w, h) in [(130, 40), (80, 24), (60, 16)] {
        for s in [Section::Status, Section::Runs, Section::History, Section::State, Section::Access, Section::Graph] {
            app.flow_section(s);
            screen(&mut app, w, h);
        }
    }

    // The state, and the runs that changed it.
    app.flow_section(Section::State);
    assert_eq!(app.flow.cells.len(), 1);
    press(&mut app, KeyCode::Enter);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(_, t) if t.starts_with("state.n"))), "{:?}", pane(&app));

    // Enter on a run: stepped through over its text, in the editor.
    app.flow_section(Section::Runs);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Editor);
    assert!(app.tabs[0].runs.as_ref().is_some_and(|r| !r.steps().is_empty()));
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains(" Runs ") && shown.contains("step 1 of") && shown.contains("Timeline"), "{shown}");
}

/// Two actions: a run of one plays none of the other.
const TWO: &str = "flow two\nstate n\n\naction bump (input) {\n  n = (state.n ?? 0) + 1\n  state.n = n\n  respond n\n}\n\naction reset (input) {\n  state.n = 0\n  respond 0\n}\n";

#[test]
fn a_run_lights_what_it_played_and_its_timeline_picks_the_step_in_the_code_and_the_graph() {
    let (mut app, state) = app();
    tab_with(&mut app, TWO);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Enter);
    let ran = route(state, &req("POST", "/trigger/two/bump", b"{}".to_vec()));
    let status = ran.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(ran)));
    app.refresh();
    ctrl(&mut app, 'r');
    let runs = app.tabs[0].runs.as_ref().expect("its runs");
    let steps = runs.steps();
    let mut lines = steps.clone();
    lines.sort();
    assert_eq!(lines, [4, 5, 6, 7], "a step for each statement it ran (the write after the answer, as it ran): {steps:?}");
    let text = match &runs.view {
        Some(Ok(v)) => v.text.clone(),
        other => panic!("no run: {other:?}"),
    };
    let line = |s: &str| text.iter().position(|l| l.contains(s)).unwrap() as u32 + 1;

    // Under the code, as in the web editor: the runs on the left, the one picked on the right,
    // its details first (it didn't fail: no Errors), tab going round to its timeline.
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Runs of two (1)") && shown.contains(" Details ") && shown.contains(" Timeline ") && !shown.contains(" Errors "), "{shown}");
    assert!(shown.contains("answered  1") && shown.contains("#0") && shown.contains("action bump"), "the run, and what it answered in its details: {shown}");
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().pane, RunPane::Details);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().pane, RunPane::Timeline);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().pane, RunPane::Details, "round the end");
    click(&mut app, Click::RunPane(RunPane::Timeline));
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().pane, RunPane::Timeline);
    for (w, h) in [(80, 24), (60, 16)] {
        screen(&mut app, w, h);
    }

    // Its values under their lines, hidden with = for the code alone, and shown again from the tab bar.
    assert!(shown.contains("⇒ 1"), "{shown}");
    press(&mut app, KeyCode::Char('='));
    let shown = screen(&mut app, 140, 40);
    assert!(!app.values && !shown.contains('⇒') && shown.contains("values hidden") && shown.contains("respond n"), "{shown}");
    click(&mut app, Click::Key(KeyCode::Char('='), KeyModifiers::NONE));
    assert!(app.values && screen(&mut app, 140, 40).contains("⇒ 1"));

    // Code and graph side by side: what it played lit, the rest of the text dimmed.
    ctrl(&mut app, 'b');
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("what the run played") && shown.contains("4 steps over") && shown.contains("step 1 of"), "{shown}");
    let g = app.tabs[0].graph.as_ref().expect("the graph of the text it ran on");
    assert!(g.view.dim);
    let lit = g.view.lit();
    assert!(lit.contains(&line("state.n = n")) && lit.contains(&line("respond n")), "{lit:?}");
    assert!(!lit.contains(&line("state.n = 0")), "the other action didn't run: {lit:?}");

    // A step picked on the timeline is the row picked on the graph, and the line marked in the code.
    let row_of = |app: &App| app.tabs[0].graph.as_ref().and_then(|g| g.view.row(g.sel)).map(|r| (r.line, r.last)).unwrap();
    for (step, &line) in steps.iter().enumerate() {
        click(&mut app, Click::Row(List::Timeline, step));
        screen(&mut app, 140, 40);
        let at = app.tabs[0].runs.as_ref().and_then(|r| r.at()).unwrap();
        assert_eq!(at, line);
        let (a, b) = row_of(&app);
        assert!((a..=b).contains(&at), "step {step} at line {at}: the graph is at {a}..{b}");
    }
    press(&mut app, KeyCode::Right);
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().step, 0, "round the end");
    // And a row clicked on the graph is that step.
    let respond = (0..app.tabs[0].graph.as_ref().unwrap().view.rows.len()).find(|&i| app.tabs[0].graph.as_ref().unwrap().view.row(i).is_some_and(|r| r.line == line("respond n"))).unwrap();
    click(&mut app, Click::Row(List::Graph, respond));
    assert_eq!(app.tabs[0].runs.as_ref().and_then(|r| r.at()), Some(line("respond n")));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains(&format!("{}▶", line("respond n"))), "the line marked: {shown}");

    // The graph alone: the row's own part of the run beside it.
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(app.show, Show::Graph);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("In this run") && shown.contains("respond n") && shown.contains("Ran"), "{shown}");
    for (w, h) in [(80, 24), (40, 12)] {
        screen(&mut app, w, h);
    }
    // Esc: back to the text, as typed.
    press(&mut app, KeyCode::Esc);
    assert!(app.tabs[0].runs.is_none());
    let shown = screen(&mut app, 140, 40);
    assert!(!app.tabs[0].graph.as_ref().unwrap().view.dim && shown.contains("as typed"), "{shown}");
}

#[test]
fn going_back_offers_what_can_be_put_back_then_a_dry_run_then_restores() {
    let (mut app, _) = bumped(1);
    // A second version, which keeps one more key, run once: the state is 3 now.
    tab_with(&mut app, &BUMP.replace("state n\n", "state n\nstate last\n").replace("+ 1", "+ 2").replace("  respond n", "  state.last = n\n  respond n"));
    ctrl(&mut app, 'd');
    // Its approval shows what it touches beside what it touches now.
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("What it keeps") && shown.contains("+ key last: write") && shown.contains("  key n: read, write"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert!(app.note.as_ref().is_some_and(|n| !n.error), "{:?}", app.note.as_ref().map(|n| &n.text));
    let c = app.client.as_ref().unwrap();
    let (lines, failed) = c.action("bump", "bump", "{}").unwrap();
    assert!(!failed, "{lines:?}");
    let n = |app: &App| app.client.as_ref().unwrap().state("bump").unwrap()["n"]["value"].as_f64();
    assert_eq!(n(&app), Some(3.0));

    app.open_flow("bump");
    app.flow_section(Section::History);
    let d = app.detail.as_ref().unwrap();
    let first = d.moments().iter().position(|m| matches!(m, Moment::Run(i) if d.runs[*i]["seq"] == d.runs[0]["seq"])).unwrap();
    app.flow.sel[Section::History as usize] = first;
    app.flow_load();
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Restore { point, sel, .. }) = &app.modal else { panic!("{:?}", app.note.as_ref().map(|n| &n.text)) };
    assert!(point.code.is_ok() && point.state.is_ok(), "{point:?}");
    assert_eq!(*sel, 0, "both, first");
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("Go back to run #") && shown.contains("Its state only"), "{shown}");

    press(&mut app, KeyCode::Enter);
    let Some(Modal::DryRun { review, refused, .. }) = &app.modal else { panic!("{:?}", app.note.as_ref().map(|n| &n.text)) };
    assert!(!refused);
    let lines = review.said();
    assert!(lines.iter().any(|(_, l)| l.contains("Version 1's code deploys again")), "{lines:#?}");
    assert!(lines.iter().any(|(t, l)| *t == Tone::Removed && l == "−   state.last = n"), "the text it puts back, as a diff: {lines:#?}");
    // What it touches: the key version 1 doesn't keep, gone, and what putting the state back does
    // to each key; and its values, now taken out and as put back put in, as the code setting them.
    let at = |text: &str| lines.iter().position(|(_, l)| l == text).unwrap_or_else(|| panic!("{text:?} in {lines:#?}"));
    at("  key n: read, write — putting the state back changes it");
    assert_eq!(lines[at("− key last: write — no longer; putting the state back drops it")].0, Tone::Removed);
    assert_eq!(lines[at("+ state.n = 1")].0, Tone::Added);
    assert_eq!(at("− state.n = 3") + 1, at("+ state.n = 1"));
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("Dry run") && shown.contains("Nothing has changed yet"), "{shown}");
    // Its code numbered, in a hunk that closes; what it keeps in another.
    assert!(shown.contains("▾ Its code: deployed now → version 1's   +1 · −3") && shown.contains("−   state.last = n") && shown.contains("▾ What it keeps"), "{shown}");
    assert_eq!(n(&app), Some(3.0), "a dry run changes nothing");
    press(&mut app, KeyCode::Char(' '));
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("▸ Its code") && !shown.contains("state.last = n"), "closed: {shown}");
    press(&mut app, KeyCode::Char('n'));
    let Some(Modal::DryRun { review, .. }) = &app.modal else { panic!() };
    let rows = review.rows();
    assert_eq!(rows[review.cursor(&rows)], super::review::Row::Head(1), "the next hunk");
    // ↓ a row at a time: past the blank one, to the next hunk (what starts it, closed: nothing
    // in it changes); ↑ back.
    press(&mut app, KeyCode::Down);
    let Some(Modal::DryRun { review, .. }) = &app.modal else { panic!() };
    assert_eq!(rows[review.cursor(&rows)], super::review::Row::Head(2));
    press(&mut app, KeyCode::Up);
    let Some(Modal::DryRun { review, .. }) = &app.modal else { panic!() };
    assert_eq!(rows[review.cursor(&rows)], super::review::Row::Head(1));
    // A click on a head opens it again, or closes it.
    let head = rows.iter().position(|r| *r == super::review::Row::Head(0)).unwrap();
    click(&mut app, Click::Row(List::Review, head));
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("▾ Its code") && shown.contains("state.last = n"), "{shown}");

    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let note = app.note.as_ref().unwrap();
    assert!(!note.error && note.text.contains("deployed again") && note.text.contains("live again"), "{}", note.text);
    assert_eq!(n(&app), Some(1.0));
}

#[test]
fn the_palette_finds_a_flows_history_and_the_call_form_sends_an_action() {
    let (mut app, _) = bumped(0);
    press(&mut app, KeyCode::Esc);
    ctrl(&mut app, 'p');
    typed(&mut app, "bump hist");
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("Find anything") && shown.contains("bump history"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert_eq!((app.page, app.flow.section), (Page::Flow, Section::History));

    press(&mut app, KeyCode::Char('c'));
    let Some(Modal::Call(form)) = &app.modal else { panic!("{:?}", app.note.as_ref().map(|n| &n.text)) };
    assert_eq!(form.items[form.sel].label(), "action bump");
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Call(form)) = &app.modal else { panic!() };
    assert!(!form.answer.is_empty() && form.answer.iter().all(|(t, _)| *t != Tone::Bad), "{:?}", form.answer);
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("Answer"), "{shown}");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.detail.as_ref().unwrap().runs.len(), 1, "the call ran, and the page shows it");
}

/// Click where something was drawn, as the mouse would.
#[test]
fn a_flows_graph_is_its_statements_wired_and_what_its_newest_run_reached() {
    let (mut app, _) = bumped(1);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Enter);
    app.flow_section(Section::Graph);
    // Every node a row at first; a (or its button on the panel's edge) the statements.
    assert!(app.flow.graph.as_ref().expect("built as its section opens").every);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Every node (") && shown.contains(" a  statements "), "{shown}");
    click(&mut app, Click::Every);
    let g = app.flow.graph.as_ref().unwrap();
    let names: Vec<String> = (0..g.rows.len()).filter_map(|i| g.row(i)).map(|r| r.name.clone()).collect();
    assert_eq!(names, ["action bump (input)", "n", "state.n", "respond"]);
    assert!(g.reached.contains(&4), "the newest run's values, by line: {:?}", g.reached);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Graph: 3 statements") && shown.contains("state.n = n") && shown.contains('●'), "{shown}");

    // What the newest run gave, by the trigger (where this run records it).
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Code { n: 4, notes, .. } if notes.iter().any(|n| n.contains('⇒')))), "{:?}", pane(&app));
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Esc);
    // A statement: its text, what it takes from and feeds, its nodes.
    press(&mut app, KeyCode::Down);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Feeds" && v.contains("state.n"))), "{:?}", pane(&app));
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(Tone::Head, t) if t.starts_with("Made of"))), "{:?}", pane(&app));

    // Every node a row again, the same statement still picked; enter edits the text there.
    press(&mut app, KeyCode::Char('a'));
    let g = app.flow.graph.as_ref().unwrap();
    assert!(g.every && g.rows.len() > 4);
    assert_eq!(g.row(app.flow.sel()).map(|r| r.line), Some(5));
    screen(&mut app, 130, 40);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Editor);
    assert_eq!(app.tabs[app.tab].buf.row, 4, "on line 5");
}

#[test]
fn the_inspector_reads_a_name_as_it_is_typed_with_its_grammar_and_an_example() {
    let (mut app, _) = app();
    tab_with(&mut app, BUMP);
    assert!(screen(&mut app, 130, 40).contains("Inspector: flow"), "the cursor on flow, at the start");
    // Typed, the name is read as it ends where the cursor is.
    app.tabs[0].buf.go(1, 99);
    typed(&mut app, "\ncollection");
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Inspector: collection") && shown.contains("Grammar") && shown.contains("header = \"collection\" ident"), "{shown}");
    // No sample is written in this form now, so its grammar comes without one's lines.
    assert!(app.tabs[0].inspected.as_ref().and_then(|i| i.usage.as_ref()).is_some_and(|u| u.example.is_none()));
    assert!(!shown.contains("from the sample"), "{shown}");
    // The cursor elsewhere, on no name: the inspector keeps it.
    app.tabs[0].buf.go(4, 1);
    assert!(screen(&mut app, 130, 40).contains("Inspector: collection"));
    // On another name, that one.
    app.tabs[0].buf.go(8, 4);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Inspector: respond") && shown.contains("respond_args ="), "{shown}");
}

#[test]
fn a_tab_shows_its_code_its_graph_beside_it_or_its_graph_alone() {
    let (mut app, _) = app();
    tab_with(&mut app, BUMP);
    let picked = |app: &App| app.tabs[0].graph.as_ref().and_then(|g| g.view.row(g.sel)).map(|r| r.name.clone()).unwrap_or_default();

    // Both: the text keeps the keys, and the graph follows the cursor.
    ctrl(&mut app, 'b');
    assert_eq!(app.show, Show::Both);
    assert!(app.typing, "the text keeps the keys");
    let shown = screen(&mut app, 130, 40);
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.every), "every node at first");
    assert!(shown.contains(" a  statements ") && !shown.lines().last().unwrap_or_default().contains("statements"), "a on the graph's edge, not the bar:\n{shown}");
    click(&mut app, Click::Every);
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| !g.view.every), "its button: the statements");
    assert!(app.typing, "the text still has the keys");
    app.tabs[0].buf.go(6, 1);
    let shown = screen(&mut app, 130, 40);
    assert_eq!(picked(&app), "state.n", "the statement at the cursor");
    assert!(shown.contains("at the cursor") && shown.contains("flow bump"), "{shown}");
    // A row clicked: the graph has the keys, ↑↓ pick, the text marks it and the cursor stays.
    click(&mut app, Click::Row(List::Graph, 1));
    assert!(app.beside_has_keys() && !app.typing, "a row clicked: the graph has the keys");
    assert_eq!((picked(&app), app.tabs[0].buf.row), ("n".to_string(), 5), "picked, the cursor where it was");
    press(&mut app, KeyCode::Down);
    let shown = screen(&mut app, 130, 40);
    assert_eq!(picked(&app), "state.n");
    assert!(shown.contains("· picking") && shown.contains("back to the text") && shown.contains("its detail"), "its keys on its edge:\n{shown}");
    let g = app.tabs[0].graph.as_ref().unwrap();
    let m = g.view.row(g.sel).unwrap().mark;
    assert_eq!((m.start.line, m.start.col, m.end.col), (6, 3, 10), "state.n, on line 6");
    press(&mut app, KeyCode::Char('a'));
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.every) && app.beside_has_keys(), "a: every node, the graph keeping the keys");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    assert!(app.typing && !app.beside_has_keys() && app.tabs[0].buf.row == 5, "esc: the text has the keys, the cursor where it was");
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("alt+g") && shown.contains("its keys"), "{shown}");
    // alt+g, even while typing: the graph's keys; enter edits the row picked there.
    app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
    assert!(app.beside_has_keys());
    assert_eq!(picked(&app), "state.n", "from the statement at the cursor");
    press(&mut app, KeyCode::Up);
    assert_eq!(picked(&app), "n");
    press(&mut app, KeyCode::Enter);
    assert!(app.typing && !app.beside_has_keys());
    assert_eq!((app.tabs[0].buf.row, app.tabs[0].buf.col), (4, 2), "at n, line 5");
    screen(&mut app, 130, 40);
    // A second click on a row edits it there.
    app.graph_beside();
    screen(&mut app, 130, 40);
    click(&mut app, Click::Row(List::Graph, 2));
    click(&mut app, Click::Row(List::Graph, 2));
    assert!(app.typing && app.tabs[0].buf.row == 5, "state.n, line 6");
    app.tabs[0].buf.go(5, 1);
    // Typed, the graph is built again.
    app.tabs[0].buf.go(7, 99);
    typed(&mut app, "\nm = n + 1");
    screen(&mut app, 130, 40);
    assert!(app.tabs[0].graph.as_ref().unwrap().view.rows.len() == 5, "a statement more");
    assert_eq!(picked(&app), "m");
    for _ in 0..11 {
        press(&mut app, KeyCode::Backspace);
    }
    assert_eq!(app.tabs[0].buf.text(), BUMP);

    // The graph alone: its keys, at the statement the cursor was in.
    ctrl(&mut app, 'b');
    assert_eq!(app.show, Show::Graph);
    assert!(!app.typing, "its keys are the graph's");
    let g = app.tabs[0].graph.as_ref().unwrap();
    let names: Vec<String> = (0..g.view.rows.len()).filter_map(|i| g.view.row(i)).map(|r| r.name.clone()).collect();
    assert_eq!(names, ["action bump (input)", "n", "state.n", "respond"]);
    assert_eq!(picked(&app), "respond");
    press(&mut app, KeyCode::Up);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("as typed") && shown.contains("state.n = n") && shown.contains("Takes from") && shown.contains('●'), "{shown}");
    press(&mut app, KeyCode::Char('a'));
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.every), "a: every node");
    assert_eq!(app.tabs[0].buf.text(), BUMP, "nothing typed into the text");
    screen(&mut app, 80, 24);

    // Enter: the text at the statement picked, the graph beside it.
    press(&mut app, KeyCode::Enter);
    assert!(app.show == Show::Both && app.typing);
    assert_eq!(app.tabs[0].buf.row, 5, "state.n = n, line 6");

    // The buttons on the tab bar pick one; esc leaves the graph for the code; a text that
    // doesn't compile says so.
    screen(&mut app, 130, 40);
    click(&mut app, Click::Show(Show::Graph));
    assert_eq!(app.show, Show::Graph);
    press(&mut app, KeyCode::Esc);
    assert!(app.show == Show::Code && app.typing);
    tab_with(&mut app, "flow broken\n\nroute GET /x (req) {\n  respond nope(\n}\n");
    click(&mut app, Click::Show(Show::Both));
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("doesn't compile"), "{shown}");
}

fn click(app: &mut App, what: Click) {
    let (rect, _) = *app.hits.borrow().iter().rev().find(|(_, c)| *c == what).unwrap_or_else(|| panic!("{what:?} wasn't drawn"));
    app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x, row: rect.y, modifiers: KeyModifiers::NONE });
}

#[test]
fn the_mouse_opens_pages_picks_rows_and_presses_buttons() {
    let (mut app, _) = bumped(1);
    press(&mut app, KeyCode::Esc);
    screen(&mut app, 130, 40);
    click(&mut app, Click::Page(Page::Flows));
    assert_eq!(app.page, Page::Flows);
    screen(&mut app, 130, 40);
    click(&mut app, Click::Row(List::Page(Page::Flows), 0));
    assert_eq!(app.page, Page::Flows, "a click picks");
    click(&mut app, Click::Row(List::Page(Page::Flows), 0));
    assert_eq!(app.page, Page::Flow, "a second click opens");
    screen(&mut app, 130, 40);
    click(&mut app, Click::Section(Section::Runs));
    assert_eq!(app.flow.section, Section::Runs);
    screen(&mut app, 130, 40);
    click(&mut app, Click::DetailTab(2));
    assert_eq!(app.flow.tab(), 2);
    // The closed shell line takes what the keys leave, and its prompt starts with user@host, which
    // is as long as the machine's (a CI runner's, runner@ip-10-0-128-198, cut it at 130): the screen
    // is as much wider, so it has room for the whole prompt on any machine.
    let shown = screen(&mut app, 130 + super::shell::who().chars().count() as u16, 40);
    assert!(shown.lines().last().is_some_and(|keys| keys.contains("refresh")), "every page but the editor's says r refreshes:\n{shown}");
    let keys = shown.lines().last().unwrap_or_default();
    let ends = if cfg!(windows) { "holon>" } else { "holon$" };
    assert!(keys.contains(&super::shell::who()) && keys.ends_with(ends), "the shell line, closed, is a prompt: who, and the folder (the crate's, in tests):\n{shown}");

    // The shell line at the bottom right opens with a click, and closes with esc.
    click(&mut app, Click::Key(KeyCode::Char('`'), KeyModifiers::CONTROL));
    assert_eq!(app.shell.as_deref(), Some(""));
    let shown = screen(&mut app, 130, 40);
    assert!(shown.lines().last().is_some_and(|line| line.contains("enter runs it in this terminal")), "{shown}");
    press(&mut app, KeyCode::Esc);
    assert!(app.shell.is_none());
    screen(&mut app, 130, 40);
    click(&mut app, Click::Key(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(matches!(app.modal, Some(Modal::Call(_))), "the keys line's buttons press their keys");
}

#[test]
fn the_text_completes_undoes_selects_and_finds() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    app.tabs[0].buf.go(4, 99);
    typed(&mut app, "\nx = sta");
    let complete = app.complete.as_ref().expect("offered as a name is typed");
    assert!(complete.items.iter().any(|c| c.label == "state"), "{:?}", complete.items.iter().map(|c| &c.label).collect::<Vec<_>>());
    let at = complete.items.iter().position(|c| c.label == "state").unwrap();
    app.take_completion(Some(at));
    assert_eq!(app.tabs[0].buf.lines[4], "x = state");
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("x = state"), "{shown}");

    ctrl(&mut app, 'z');
    assert_eq!(app.tabs[0].buf.lines[4], "x = sta", "undo takes back the completion");
    ctrl(&mut app, 'z');
    assert_eq!(app.tabs[0].buf.lines[4], "x =", "then a word at a time");
    for _ in 0..3 {
        ctrl(&mut app, 'z');
    }
    assert_eq!(app.tabs[0].buf.text(), BEATS, "and then the line");
    ctrl(&mut app, 'y');
    assert_ne!(app.tabs[0].buf.text(), BEATS);

    // Shift+arrows select; ctrl+c copies; ctrl+f finds and selects.
    app.tabs[0].buf.go(1, 1);
    app.key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));
    ctrl(&mut app, 'c');
    assert_eq!(app.clipboard, "flow beats");
    assert_eq!(app.to_clipboard.as_deref(), Some("flow beats"), "for the terminal's clipboard too");
    ctrl(&mut app, 'f');
    typed(&mut app, "every");
    assert_eq!(app.tabs[0].buf.selected().as_deref(), Some("every"));
    assert_eq!(app.tabs[0].buf.row, 1);
    press(&mut app, KeyCode::Esc);
    assert!(app.find.is_none());
}

#[test]
fn a_panels_keys_sit_on_its_bottom_edge_and_the_bar_keeps_those_that_work_anywhere() {
    let (mut app, _) = app();
    press(&mut app, KeyCode::Char('6'));
    let shown = screen(&mut app, 140, 30);
    let lines: Vec<&str> = shown.lines().collect();
    let edge = lines.iter().position(|l| l.contains("open a copy")).unwrap_or_else(|| panic!("{shown}"));
    assert!(lines[edge].contains('╰') && edge < lines.len() - 2, "on the samples' bottom edge: {shown}");
    let bar = lines[lines.len() - 1];
    assert!(bar.contains("quit") && bar.contains("command") && bar.contains("keys") && !bar.contains("open a copy"), "{bar}");
    // A key on the edge is a button too.
    press(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.modal, Some(Modal::Help { .. })));
}

#[test]
fn every_page_draws_narrow_and_wide_and_without_a_server() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    for page in [Page::Overview, Page::Approvals, Page::Flows, Page::Editor, Page::Library, Page::Admin] {
        app.page = page;
        for (w, h) in [(80, 24), (140, 45), (40, 12)] {
            screen(&mut app, w, h);
        }
    }
    let wide = screen(&mut app, 140, 45);
    assert!(wide.contains("Overview") && wide.contains("Library"), "the sidebar, when wide: {wide}");
    let narrow = screen(&mut app, 80, 24);
    assert!(!narrow.contains(" 1  Overview"), "folded when narrow: {narrow}");

    let mut offline = App::new(None);
    offline.offline = Some("no holon serve running".into());
    let shown = screen(&mut offline, 120, 30);
    assert!(shown.contains("No server reached") && shown.contains("no server: editing files only"), "{shown}");
    offline.open_sample("hello-api");
    let shown = screen(&mut offline, 120, 30);
    assert!(shown.contains("hello-api (sample)") && shown.contains(r#"route.get("/hello/:name""#), "{shown}");
    ctrl(&mut offline, 'd');
    assert!(offline.note.as_ref().is_some_and(|n| n.error && n.text.contains("no server")));
}

#[test]
fn commands_open_files_and_save_them() {
    let dir = std::env::temp_dir().join(format!("holon-editor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("fresh.holon");
    let _ = std::fs::remove_file(&file);
    let mut app = App::new(None);
    press(&mut app, KeyCode::Char(':'));
    typed(&mut app, &format!("open {}\n", file.display()));
    assert_eq!(app.tabs.len(), 1, "{:?}", app.note.as_ref().map(|n| &n.text));
    assert_eq!(app.tabs[0].buf.text(), "flow fresh\n", "a new file starts as a flow named for it");
    assert!(app.tabs[0].dirty());
    ctrl(&mut app, 's');
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "flow fresh\n");
    assert!(!app.tabs[0].dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Put `text` in the open input dialog, in place of what it holds, and press enter.
fn answer(app: &mut App, text: &str) {
    let Some(Modal::Input { value, .. }) = app.modal.as_mut() else { panic!("no input dialog: {:?}", app.note.as_ref().map(|n| &n.text)) };
    *value = text.to_string();
    press(app, KeyCode::Enter);
}

#[test]
fn a_draft_opens_in_a_new_tab_and_its_first_save_asks_for_a_file() {
    let dir = std::env::temp_dir().join(format!("holon-draft-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(None);
    ctrl(&mut app, 'n');
    assert_eq!((app.page, app.tabs.len(), app.tabs[0].name.as_str()), (Page::Editor, 1, "draft 1"));
    assert!(app.typing && app.tabs[0].draft && app.tabs[0].dirty());
    assert_eq!(app.tabs[0].buf.text(), holon_author::commands::STARTER);
    // n on the page keys starts another, numbered after the first.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.tabs[1].name, "draft 2");

    ctrl(&mut app, 's');
    let Some(Modal::Input { value, .. }) = &app.modal else { panic!("no dialog") };
    assert_eq!(value, "my-flow.holon.ts", "named for its flow");
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("Save draft 2 as"), "{shown}");
    // A file that is there already is never written over from here.
    let there = dir.join("there.holon");
    std::fs::write(&there, "flow there\n").unwrap();
    answer(&mut app, &there.display().to_string());
    assert!(app.note.as_ref().is_some_and(|n| n.error && n.text.contains("there already")));
    assert_eq!(std::fs::read_to_string(&there).unwrap(), "flow there\n");
    assert!(app.tabs[1].draft);

    let file = dir.join("mine.holon.ts");
    let _ = std::fs::remove_file(&file);
    ctrl(&mut app, 's');
    answer(&mut app, &file.display().to_string());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), holon_author::commands::STARTER);
    let tab = &app.tabs[1];
    assert!(!tab.draft && !tab.dirty() && tab.name == "mine.holon.ts" && tab.path.as_deref() == Some(file.as_path()));
    // Saved, it saves to its file without asking.
    typed(&mut app, "// more\n");
    ctrl(&mut app, 's');
    assert!(app.modal.is_none());
    assert!(std::fs::read_to_string(&file).unwrap().starts_with("// more\n"));
    // The next draft takes the first number free.
    ctrl(&mut app, 'n');
    assert_eq!(app.tabs[2].name, "draft 2");
    // ctrl+t opens one in a new tab too, as a browser does, and so does the tab bar's +.
    ctrl(&mut app, 't');
    assert_eq!((app.tabs.len(), app.tab, app.tabs[3].name.as_str()), (4, 3, "draft 3"));
    screen(&mut app, 120, 30);
    click(&mut app, Click::Key(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert_eq!((app.tabs.len(), app.tab, app.tabs[4].name.as_str()), (5, 4, "draft 4"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A run that failed opens on its Errors: why, and the line that gave nothing for it, once.
#[test]
fn a_run_that_failed_opens_on_why_and_on_which_line() {
    let mut app = App::new(None);
    ctrl(&mut app, 't');
    let view = holon_author::agent::RunView {
        head: "run 3: GET /nearest/kittne → 500".into(),
        no_steps: false,
        text: vec!["flow nearest".into(), "".into(), "route GET /nearest/:word (req) {".into(), "  best = closest(req.params.word)".into(), "  respond best".into(), "}".into()],
        notes: [(4, vec!["⇒ nothing: closest gave nothing: out of fuel".to_string()]), (5, vec!["⇒ nothing: closest gave nothing: out of fuel".to_string()])].into_iter().collect(),
        after: vec!["answered 500: ".into(), "failed: closest gave nothing".into()],
        steps: vec![],
    };
    let entry = serde_json::json!({ "seq": 3, "kind": "request", "method": "GET", "url": "/nearest/kittne", "status": 500, "duration_ms": 2.5, "error": "closest gave nothing" });
    let mut runs = Runs { slug: "nearest".into(), entries: vec![entry], sel: 0, view: Some(Ok(view)), step: 0, pane: RunPane::Details };
    assert_eq!(runs.errors(), [(None, "closest gave nothing".to_string()), (Some(4), "closest gave nothing: out of fuel".to_string())]);
    assert_eq!(runs.panes(), [RunPane::Details, RunPane::Errors, RunPane::Timeline]);
    runs.pane = RunPane::Errors;
    app.tabs[0].runs = Some(runs);
    app.typing = false;
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains(" Errors (2) ") && shown.contains("line 4") && shown.contains("best = closest(req.params.word)") && shown.contains("out of fuel"), "{shown}");
    assert!(shown.contains("GET /nearest/kittne") && shown.contains("err"), "the run's row says it failed: {shown}");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().pane, RunPane::Timeline);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("It ran nothing this text says.") || shown.contains("recorded no timing"), "{shown}");
    for (w, h) in [(80, 24), (60, 16), (40, 12)] {
        screen(&mut app, w, h);
    }
}

#[test]
fn the_admin_page_invites_people_makes_keys_and_binds_folders_and_databases() {
    let (mut app, _) = app();
    press(&mut app, KeyCode::Char('7'));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("This page is the server's admins'") && shown.contains("no accounts"), "{shown}");

    let mut app = admin_app();
    press(&mut app, KeyCode::Char('7'));
    assert_eq!(app.page, Page::Admin);
    let shown = screen(&mut app, 140, 40);
    for part in ["People", "API keys", "Folders", "Databases", "Secrets", "Server"] {
        assert!(shown.contains(part), "{part}: {shown}");
    }

    // People: invite, then change the role; the link is shown once, and copied.
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "ana editor ana@example.com");
    let Some(Modal::Lines { title, .. }) = &app.modal else { panic!("no link: {:?}", app.note.as_ref().map(|n| &n.text)) };
    assert_eq!(title, "ana is invited");
    assert!(app.to_clipboard.is_some());
    press(&mut app, KeyCode::Esc);
    let ana = app.admin.users.iter().position(|u| u["name"] == "ana").expect("ana is listed");
    app.admin_pick(List::Admin, ana, false);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("ana@example.com") && shown.contains("invited"), "{shown}");
    press(&mut app, KeyCode::Enter);
    answer(&mut app, "viewer");
    assert!(app.admin.users.iter().any(|u| u["name"] == "ana" && u["role"] == "viewer"), "{:?}", app.note.as_ref().map(|n| &n.text));

    // API keys: made, shown once, then revoked.
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "nightly viewer");
    assert!(matches!(&app.modal, Some(Modal::Lines { title, .. }) if title == "nightly: an API key"), "{:?}", app.note.as_ref().map(|n| &n.text));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.admin.tokens.len(), 1);
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Enter);
    assert!(app.admin.tokens.is_empty(), "{:?}", app.note.as_ref().map(|n| &n.text));

    // Folders: bound with holon folder add's flags, then unbound.
    let dir = std::env::temp_dir().join(format!("holon-admin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, &format!("inbox {} --read-only", dir.display()));
    let f = app.admin.folders.first().unwrap_or_else(|| panic!("not bound: {:?}", app.note.as_ref().map(|n| &n.text)));
    assert_eq!((f["name"].as_str(), f["access"].as_str()), (Some("inbox"), Some("read")));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("inbox") && shown.contains("every flow"), "{shown}");
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Enter);
    assert!(app.admin.folders.is_empty(), "{:?}", app.note.as_ref().map(|n| &n.text));

    // Databases: a SQLite file, its schema, its tables.
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, &format!("shop {} --schema create table if not exists items (id integer primary key)", dir.join("shop.db").display()));
    assert_eq!(app.admin.databases.len(), 1, "{:?}", app.note.as_ref().map(|n| &n.text));
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Lines { lines, .. }) = &app.modal else { panic!("no tables: {:?}", app.note.as_ref().map(|n| &n.text)) };
    assert!(lines.iter().any(|(_, l)| l == "items"), "{lines:?}");
    press(&mut app, KeyCode::Esc);

    // Secrets and the server: their panes read the server.
    press(&mut app, KeyCode::Tab);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Read from the environment"), "{shown}");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Signing in through an identity provider"), "{shown}");
    for (w, h) in [(80, 24), (40, 12)] {
        screen(&mut app, w, h);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A secret is set from the flow's Access section as the web editor sets it: a value typed and
/// sealed, a `HOLON_SECRET_…` name the server reads (its own name's picked first), another name,
/// or a Vault field; never a value shown.
#[test]
fn a_secret_is_set_as_a_value_a_name_the_server_reads_or_a_vault_field() {
    use super::flow::{Access, Source, access_rows};
    let mut state = test_state();
    let sources = crate::secret_sources::Sources::of(&[("HOLON_SECRET_SIGNING_KEY", "dummy-signing-key"), ("HOLON_SECRET_OTHER_KEY", "dummy-other-key")]);
    state.secrets = std::sync::Arc::new(crate::secrets::SecretStore::memory().with_sources(sources));
    let state: &'static AppState = Box::leak(Box::new(state));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, None))));
    tab_with(&mut app, include_str!("../../../holon-author/samples/sign-payload.holon.ts"));
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Enter);
    app.open_flow("sign-payload");
    app.flow_section(Section::Access);
    let at = access_rows(app.detail.as_ref().unwrap()).iter().position(|a| matches!(a, Access::Secret { name, .. } if name == "signing_key")).expect("the secret it reads");
    let source = |app: &App| app.detail.as_ref().unwrap().secrets["secrets"].as_array().into_iter().flatten().find(|s| s["name"] == "signing_key").and_then(|s| s["source"].as_str()).map(str::to_string);
    let open = |app: &mut App| {
        app.flow_pick(List::Section, at);
        press(app, KeyCode::Enter);
        screen(app, 130, 40);
        let Some(Modal::SecretSource(c)) = &app.modal else { panic!("no choice: {:?}", app.note.as_ref().map(|n| &n.text)) };
        (c.choices(), c.sel)
    };

    // Not set: the server's names offered, the one its name suggests picked; enter sets it from there.
    assert_eq!(source(&app), None);
    let (choices, sel) = open(&mut app);
    assert_eq!(choices, [Source::Value, Source::Env("HOLON_SECRET_OTHER_KEY".into()), Source::Env("HOLON_SECRET_SIGNING_KEY".into()), Source::OtherEnv, Source::Vault]);
    assert_eq!(choices[sel], Source::Env("HOLON_SECRET_SIGNING_KEY".into()));
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Where sign-payload gets signing_key") && shown.contains("Not set") && shown.contains("← its name"), "{shown}");
    assert!(!shown.contains("dummy-"), "names only, never a value: {shown}");
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none(), "{:?}", app.note.as_ref().map(|n| &n.text));
    assert!(app.note.as_ref().is_some_and(|n| !n.error && n.text == "set signing_key (v1) for sign-payload, read from env:HOLON_SECRET_SIGNING_KEY"));
    assert_eq!(source(&app).as_deref(), Some("env:HOLON_SECRET_SIGNING_KEY"));

    // Set: where it comes from now is picked. A value instead, typed as dots.
    let (choices, sel) = open(&mut app);
    assert_eq!(choices[sel], Source::Env("HOLON_SECRET_SIGNING_KEY".into()));
    assert!(screen(&mut app, 130, 40).contains("← now"));
    press(&mut app, KeyCode::Home);
    for _ in 0..sel {
        press(&mut app, KeyCode::Up);
    }
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Input { masked: true, .. })));
    typed(&mut app, "a-typed-signing-key");
    assert!(!screen(&mut app, 130, 40).contains("a-typed-signing-key"), "typed as dots");
    press(&mut app, KeyCode::Enter);
    assert_eq!(source(&app).as_deref(), Some("local"));

    // Another name, typed: refused when the server can't read it, set when it can.
    let (choices, _) = open(&mut app);
    click(&mut app, Click::Row(List::SecretSource, choices.iter().position(|c| *c == Source::OtherEnv).unwrap()));
    let Some(Modal::Input { value, masked: false, .. }) = &app.modal else { panic!("no name asked") };
    assert_eq!(value, "HOLON_SECRET_", "started for you");
    answer(&mut app, "HOLON_SECRET_NOT_THERE");
    assert!(app.note.as_ref().is_some_and(|n| n.error && n.text.contains("isn't set")), "{:?}", app.note.as_ref().map(|n| &n.text));
    assert_eq!(source(&app).as_deref(), Some("local"), "left as it was");
    let (choices, _) = open(&mut app);
    click(&mut app, Click::Row(List::SecretSource, choices.iter().position(|c| *c == Source::OtherEnv).unwrap()));
    answer(&mut app, "HOLON_SECRET_OTHER_KEY");
    assert_eq!(source(&app).as_deref(), Some("env:HOLON_SECRET_OTHER_KEY"));

    // A Vault field: its form checked by the server.
    let (choices, _) = open(&mut app);
    click(&mut app, Click::Row(List::SecretSource, choices.iter().position(|c| *c == Source::Vault).unwrap()));
    answer(&mut app, "not a vault path");
    assert!(app.note.as_ref().is_some_and(|n| n.error && n.text.contains("vault:")), "{:?}", app.note.as_ref().map(|n| &n.text));
    for (w, h) in [(80, 24), (40, 12)] {
        screen(&mut app, w, h);
    }
}

/// An unrecorded action's runs aren't in the runs list: the flow page says them, counted, a line
/// for the handler in its latest runs and a count in the runs section's title
/// (docs/L4s/request-path-L4.md, C5).
#[test]
fn a_flows_unrecorded_handlers_show_counted_beside_its_runs() {
    let (mut app, _) = app();
    let text = "flow peek\nstate n\n\naction peek (input) unrecorded {\n  respond state.n ?? 0\n}\n\naction bump (input) {\n  state.n = (state.n ?? 0) + 1\n  respond \"bumped\"\n}\n";
    tab_with(&mut app, text);
    ctrl(&mut app, 'd');
    let Some(Modal::Approve { .. }) = &app.modal else { panic!("no dialog: {:?}", app.note.as_ref().map(|n| &n.text)) };
    let shown = screen(&mut app, 140, 44);
    assert!(shown.contains("+ log action peek: not recorded"), "the approval says it: {shown}");
    press(&mut app, KeyCode::Enter);
    assert!(app.note.as_ref().is_some_and(|n| !n.error), "{:?}", app.note.as_ref().map(|n| &n.text));
    let c = app.client.as_ref().unwrap();
    for _ in 0..3 {
        assert!(!c.action("peek", "peek", "{}").unwrap().1);
    }
    assert!(!c.action("peek", "bump", "{}").unwrap().1);
    app.client.as_mut().unwrap().refresh().unwrap();
    app.open_flow("peek");
    let shown = screen(&mut app, 140, 44);
    assert!(shown.contains("action peek  not recorded: 3 runs"), "{shown}");
    app.flow_section(Section::Runs);
    let shown = screen(&mut app, 140, 44);
    assert!(shown.contains("Runs, newest first (1, +3 unrecorded)"), "{shown}");
}
