//! `holon editor` driven by its keys against the real router, in-process, and drawn to a test
//! screen: what a person sees and does there.

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;

use super::app::{App, Click, List, Modal, Page, RunPane, Runs, Show, Tone};
use super::client::{Client, Moment};
use super::flow::{Row, Section};
use super::admin::Part;
use super::graph::Level;
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

/// A clock flow: its beat in its state, every 5 seconds.
const BEATS: &str = "import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"beats\")\n\nevery(\"5s\", () => {\n  state.beats = (state.beats ?? 0) + 1\n})\n";
/// A flow run by hand: its action on line 6, `const n` on 7, `state.n = n` on 8, `return n` on 9.
const BUMP: &str = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"bump\")\n\nexport const bump = action((input) => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  return n\n})\n";

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
    assert!(shown.contains("It may") && shown.contains("m adds a note"), "{shown}");
    // A note kept with it: typed, then the dialog is back, saying it; esc leaves it as it was.
    press(&mut app, KeyCode::Char('m'));
    typed(&mut app, "for the pilot
");
    assert!(matches!(&app.modal, Some(Modal::Approve { staged, .. }) if staged.note() == Some("for the pilot")));
    press(&mut app, KeyCode::Char('m'));
    press(&mut app, KeyCode::Esc);
    assert!(screen(&mut app, 120, 40).contains("Note: for the pilot"));
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let events = app.client.as_ref().unwrap().get("/deployments/beats/events").unwrap();
    assert!(events.to_string().contains("for the pilot"), "the note is on the deploy's event: {events}");
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
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.tabs[0].slug.as_deref(), Some("beats"));
    let renamed = BEATS.replace("flow(\"beats\")", "flow(\"pulse\")");
    app.tabs[0].buf.replace(&renamed);
    app.tabs[0].check = super::app::Check::of(&renamed);
    assert_eq!(app.tabs[0].target().as_deref(), Some("pulse"), "its runs, its grant: the new name's");
    ctrl(&mut app, 'd');
    let Some(Modal::Approve { slug, .. }) = &app.modal else { panic!("{:?}", app.note.as_ref().map(|n| &n.text)) };
    assert_eq!(slug, "pulse");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("deploys as a new flow") && shown.contains("\"beats\", which this tab deployed before, stays"), "{shown}");
    press(&mut app, KeyCode::Char('t'));
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
    // In the clock's body, after its write.
    app.tabs[0].buf.go(7, 99);
    typed(&mut app, "\nconst x = 1 +");
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
    app.client.as_ref().unwrap().save("beats", &plan, BEATS, None).unwrap();
    app.refresh();
    let waiting = app.waiting();
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].1.starts_with("to deploy it for the first time"), "{waiting:?}");
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("Waiting for you (1)"), "the overview counts what waits: {shown}");
    press(&mut app, KeyCode::Char('2'));
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("beats") && shown.contains("first time"), "{shown}");
    // e opens it in the editor; enter approves it here, as the review beside the list shows it.
    press(&mut app, KeyCode::Char('e'));
    assert_eq!(app.page, Page::Editor);
    press(&mut app, KeyCode::Esc);
    app.go(Page::Approvals);
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Approvals);
    assert!(app.waiting().is_empty(), "{:?}", app.waiting());
}

/// Deploy BUMP from a tab, and run its action `times` times.
fn bumped(times: usize) -> (App, &'static AppState) {
    let (mut app, state) = app();
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    for _ in 0..times {
        let ran = route(state, &req("POST", "/trigger/bump/bump", b"{}".to_vec()));
        let status = ran.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(ran)));
    }
    app.refresh();
    (app, state)
}

/// A flow that writes lines at three levels, and the page of one that has run it twice.
const LOUD: &str = "import { action, flow } from \"holon\"\n\nflow(\"loud\")\n\nexport const shout = action((input) => {\n  console.log(\"asked\", input.who)\n  console.warn(\"careful\")\n  console.debug(\"quiet\")\n})\n";

fn loud_page() -> App {
    let (mut app, state) = app();
    tab_with(&mut app, LOUD);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    for who in ["ada", "bob"] {
        let ran = route(state, &req("POST", "/trigger/loud/shout", format!("{{\"who\":\"{who}\"}}").into_bytes()));
        let status = ran.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(ran)));
    }
    app.refresh();
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Flow);
    app
}

fn texts(app: &App) -> Vec<String> {
    app.flow.lines.iter().map(|l| l["text"].as_str().unwrap_or_default().to_string()).collect()
}

#[test]
fn a_flows_log_lines_are_a_section_of_their_own_beside_its_runs() {
    let mut app = loud_page();
    // The Logs open on the newest line, as a terminal does, each with its level and its run.
    app.flow_section(Section::Logs);
    assert_eq!(texts(&app), ["asked ada", "careful", "quiet", "asked bob", "careful", "quiet"]);
    assert_eq!(app.flow.sel(), 5, "the newest is picked");
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Log lines, oldest first (6)") && shown.contains("asked bob") && shown.contains("WARN") && shown.contains("DEBUG") && shown.contains(" Line "), "{shown}");
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Run" && v == "#1")), "{:?}", pane(&app));
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(_, t) if t == "quiet")), "{:?}", pane(&app));
    press(&mut app, KeyCode::Up);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Level" && v == "WARN")), "the pane follows the pick: {:?}", pane(&app));

    // w: warnings and louder, then errors only (none), then every line again.
    press(&mut app, KeyCode::Char('w'));
    assert_eq!(texts(&app), ["careful", "careful"]);
    assert!(screen(&mut app, 130, 40).contains("warnings and louder"));
    press(&mut app, KeyCode::Char('w'));
    assert!(app.flow.lines.is_empty());
    assert!(screen(&mut app, 130, 40).contains("No lines yet"));
    press(&mut app, KeyCode::Char('w'));
    assert_eq!(app.flow.lines.len(), 6);

    // u: the lines up to the end of the picked line's run, then its own, then every run's.
    app.flow.sel[Section::Logs as usize] = 1;
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.flow.log_run, Some((0, false)));
    assert_eq!(texts(&app), ["asked ada", "careful", "quiet"]);
    assert!(screen(&mut app, 130, 40).contains("up to run #0"));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.flow.log_run, Some((0, true)));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.flow.log_run, None);
    assert_eq!(app.flow.lines.len(), 6);

    // F pauses it, and a new line then leaves the pick where it is; Enter steps through the run.
    press(&mut app, KeyCode::Char('F'));
    assert!(app.flow.log_paused);
    assert!(screen(&mut app, 130, 40).contains("paused"));
    press(&mut app, KeyCode::Char('F'));
    assert!(!app.flow.log_paused);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Editor, "stepped through the run the line is of");
}

#[test]
fn a_runs_lines_are_one_key_away() {
    let mut app = loud_page();
    app.flow_section(Section::Runs);
    press(&mut app, KeyCode::Down);
    let seq = app.detail.as_ref().and_then(|d| d.shown_runs("").get(app.flow.sel()).and_then(|&i| d.runs[i]["seq"].as_u64())).unwrap();
    press(&mut app, KeyCode::Char('L'));
    assert_eq!(app.flow.section, Section::Logs);
    assert_eq!(app.flow.log_run, Some((seq, false)));
    assert!(app.flow.lines.iter().all(|l| l["run"].as_u64().is_some_and(|r| r <= seq)), "{:?}", app.flow.lines);
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
        for s in [Section::Status, Section::Runs, Section::Logs, Section::History, Section::State, Section::Access, Section::Settings, Section::Graph] {
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
const TWO: &str = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"two\")\n\nexport const bump = action((input) => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  return n\n})\n\nexport const reset = action((input) => {\n  state.n = 0\n  return 0\n})\n";

#[test]
fn a_run_lights_what_it_played_and_its_timeline_picks_the_step_in_the_code_and_the_graph() {
    let (mut app, state) = app();
    tab_with(&mut app, TWO);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    let ran = route(state, &req("POST", "/trigger/two/bump", b"{}".to_vec()));
    let status = ran.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(ran)));
    app.refresh();
    ctrl(&mut app, 'r');
    let runs = app.tabs[0].runs.as_ref().expect("its runs");
    // A run is shown over the text its view prints from the deployed document (agent::run_view),
    // laid out as fmt writes it: the action's line, then its statements.
    let steps = runs.steps();
    let mut lines = steps.clone();
    lines.sort();
    assert_eq!(lines, [6, 7, 8, 9], "a step for each statement it ran (the write after the answer, as it ran): {steps:?}");
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
    assert!(!app.values && !shown.contains('⇒') && shown.contains("values hidden") && shown.contains("return n"), "{shown}");
    click(&mut app, Click::Key(KeyCode::Char('='), KeyModifiers::NONE));
    assert!(app.values && screen(&mut app, 140, 40).contains("⇒ 1"));

    // Code and graph side by side: what it played lit, the rest of the text dimmed.
    ctrl(&mut app, 'b');
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("what the run played") && shown.contains("4 steps over") && shown.contains("step 1 of"), "{shown}");
    let g = app.tabs[0].graph.as_ref().expect("the graph of the text it ran on");
    assert!(g.view.dim);
    let lit = g.view.lit();
    assert!(lit.contains(&line("state.n = n")) && lit.contains(&line("return n")), "{lit:?}");
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
    let respond = (0..app.tabs[0].graph.as_ref().unwrap().view.rows.len()).find(|&i| app.tabs[0].graph.as_ref().unwrap().view.row(i).is_some_and(|r| r.line == line("return n"))).unwrap();
    click(&mut app, Click::Row(List::Graph, respond));
    assert_eq!(app.tabs[0].runs.as_ref().and_then(|r| r.at()), Some(line("return n")));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains(&format!("{}▶", line("return n"))), "the line marked: {shown}");

    // The graph alone: the row's own part of the run beside it.
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(app.show, Show::Graph);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("In this run") && shown.contains("return n") && shown.contains("Ran"), "{shown}");
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
    tab_with(&mut app, &BUMP.replace("+ 1", "+ 2").replace("  return n", "  state.last = n\n  return n"));
    ctrl(&mut app, 'd');
    // Its approval shows what it touches beside what it touches now.
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("What it keeps") && shown.contains("+ key last: write") && shown.contains("  key n: read, write"), "{shown}");
    press(&mut app, KeyCode::Char('t'));
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
    assert!(shown.contains("▾ Its code: deployed now → version 1's   +1 · −2") && shown.contains("−   state.last = n") && shown.contains("▾ What it keeps"), "{shown}");
    assert_eq!(n(&app), Some(3.0), "a dry run changes nothing");
    press(&mut app, KeyCode::Char(' '));
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("▸ Its code") && !shown.contains("state.last = n"), "closed: {shown}");
    press(&mut app, KeyCode::Char('n'));
    let Some(Modal::DryRun { review, .. }) = &app.modal else { panic!() };
    let rows = review.rows();
    assert_eq!(rows[review.cursor(&rows)], super::review::Row::Head(1), "the next hunk: its handlers");
    assert!(lines.iter().any(|(t, l)| *t == Tone::Warn && l == "~ handler action bump"), "the handler whose code changes: {lines:#?}");
    // ↓ a row at a time from the next (what starts it, closed: nothing in it changes): past the
    // blank one, to the hunk after it; ↑ back.
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Down);
    let Some(Modal::DryRun { review, .. }) = &app.modal else { panic!() };
    assert_eq!(rows[review.cursor(&rows)], super::review::Row::Head(3));
    press(&mut app, KeyCode::Up);
    let Some(Modal::DryRun { review, .. }) = &app.modal else { panic!() };
    assert_eq!(rows[review.cursor(&rows)], super::review::Row::Head(2));
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

/// The palette finds a run by its number (`1`, `#1`, `run 1`), first; the latest runs the
/// server's summary names; and what waits for a person, each opening where it is decided.
#[test]
fn the_palette_finds_a_run_by_its_number_the_latest_runs_and_what_waits() {
    let (mut app, _) = bumped(2);
    press(&mut app, KeyCode::Esc);
    let plan = holon_author::agent::plan(BEATS).unwrap();
    app.client.as_ref().unwrap().save("beats", &plan, BEATS, None).unwrap();
    app.refresh();

    let labels = |app: &App, q: &str| super::palette::items(app, q).into_iter().map(|i| (i.label, i.kind)).collect::<Vec<_>>();
    let all = labels(&app, "");
    assert!(all.contains(&("run #1 of bump".into(), "run")) && all.contains(&("run #0 of bump".into(), "run")), "the summary's latest runs: {all:?}");
    assert!(all.iter().any(|(l, k)| l.starts_with("beats waits to deploy it for the first time") && *k == "approvals"), "{all:?}");
    for q in ["1", "#1", "run 1"] {
        let found = labels(&app, q);
        assert_eq!(found[0], ("run #1 of bump".into(), "run"), "{q}: {found:?}");
        assert_eq!(found.iter().filter(|(l, _)| l == "run #1 of bump").count(), 1, "not twice: {found:?}");
    }

    ctrl(&mut app, 'p');
    typed(&mut app, "#1");
    press(&mut app, KeyCode::Enter);
    assert_eq!((app.page, app.flow.section), (Page::Flow, Section::Runs));
    let d = app.detail.as_ref().unwrap();
    assert_eq!(d.shown_runs("").get(app.flow.sel()).map(|&i| d.runs[i]["seq"].as_u64()), Some(Some(1)), "run #1 picked");

    // A run no flow has: the flow opens on its runs, and says where that run is.
    ctrl(&mut app, 'p');
    typed(&mut app, "run 99");
    press(&mut app, KeyCode::Enter);
    assert!(note(&app).contains("run #99 isn't among the newest runs read"), "{}", note(&app));

    ctrl(&mut app, 'p');
    typed(&mut app, "beats waits");
    press(&mut app, KeyCode::Enter);
    assert_eq!((app.page, app.approvals_part), (Page::Approvals, super::approvals::Part::Deploys));
    assert_eq!(app.waiting()[app.sel[1]].0, "beats");
}

/// The keys dialog says every line of the help whole: a long one goes on under itself, none cut.
#[test]
fn the_keys_dialog_cuts_no_line() {
    let (mut app, _) = app();
    press(&mut app, KeyCode::Esc);
    app.modal = Some(Modal::Help { scroll: 0 });
    let mut seen = String::new();
    for scroll in (0..200).step_by(20) {
        app.modal = Some(Modal::Help { scroll });
        seen += &screen(&mut app, 140, 45);
    }
    let words: std::collections::HashSet<&str> = seen.split_whitespace().collect();
    for (_, what) in super::ui::modal::HELP {
        for word in what.split_whitespace() {
            assert!(words.contains(word), "{word:?} of {what:?} isn't shown whole");
        }
    }
}

/// A saved flow is deleted from Flows, asked first, as the web's Flows page does: n keeps it, enter
/// deletes it. A flow that is deployed and not saved says there is nothing saved to delete.
#[test]
fn a_saved_flow_is_deleted_from_flows_once_confirmed() {
    let (mut app, _) = bumped(0);
    press(&mut app, KeyCode::Esc);
    let plan = holon_author::agent::plan(BEATS).unwrap();
    app.client.as_ref().unwrap().save("beats", &plan, BEATS, None).unwrap();
    app.refresh();
    press(&mut app, KeyCode::Char('3'));
    let at = |app: &App, name: &str| app.flow_names().iter().position(|n| n == name).unwrap();
    app.sel[2] = at(&app, "beats");

    press(&mut app, KeyCode::Delete);
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("Delete the saved flow \"beats\"?") && shown.contains("can't be undone"), "{shown}");
    press(&mut app, KeyCode::Char('n'));
    assert!(app.modal.is_none() && app.client.as_ref().unwrap().saved("beats").is_some(), "n keeps it");

    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Enter);
    assert!(app.client.as_ref().unwrap().saved("beats").is_none(), "{}", note(&app));
    assert!(!app.flow_names().contains(&"beats".to_string()));
    assert!(note(&app).contains("deleted the saved flow \"beats\""), "{}", note(&app));

    // bump: deployed; its saved record, if its deploy kept one, goes, and its deployment stays.
    app.sel[2] = at(&app, "bump");
    let saved = app.client.as_ref().unwrap().saved("bump").is_some();
    press(&mut app, KeyCode::Char('D'));
    if saved {
        let shown = screen(&mut app, 120, 30);
        assert!(shown.contains("Its deployment stays and keeps running"), "{shown}");
        press(&mut app, KeyCode::Enter);
    } else {
        assert!(app.modal.is_none() && note(&app).contains("isn't saved on the server"), "{}", note(&app));
    }
    assert!(app.client.as_ref().unwrap().saved("bump").is_none());
    assert!(app.client.as_ref().unwrap().deployment("bump").is_some(), "its deployment stays");
}

/// Click where something was drawn, as the mouse would.
#[test]
fn a_flows_graph_is_its_statements_wired_and_what_its_newest_run_reached() {
    let (mut app, _) = bumped(1);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Enter);
    app.flow_section(Section::Graph);
    // Every node a row at first; a (or its switch on the panel's edge) the statements.
    assert_eq!(app.flow.graph.as_ref().expect("built as its section opens").level, Level::Nodes);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Nodes: ") && shown.contains(" a  nodes ") && shown.contains(" Graph "), "{shown}");
    click(&mut app, Click::Every);
    let g = app.flow.graph.as_ref().unwrap();
    let names: Vec<String> = (0..g.rows.len()).filter_map(|i| g.row(i)).map(|r| r.name.clone()).collect();
    assert_eq!(names, ["action bump", "n", "state.n", "return"], "its trigger named as its runs are");
    // The newest run's values are by the lines of the text its view prints (agent::run_view), laid
    // out as fmt writes it, as this text is: the write's line among them.
    let write = (0..g.rows.len()).filter_map(|i| g.row(i)).find(|r| r.name == "state.n").unwrap().line;
    assert!(g.reached.contains(&write), "the newest run's values, by line: {:?}", g.reached);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Statements: 3 of") && shown.contains("state.n = n") && shown.contains('●'), "{shown}");

    // The newest run of the trigger, found by its name, beside the trigger's line.
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Newest run" && v.starts_with("#0"))), "{:?}", pane(&app));
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Code { n: 6, .. })), "{:?}", pane(&app));
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Esc);
    // A statement: its text, what it takes from and feeds, its nodes.
    press(&mut app, KeyCode::Down);
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Field(k, v, _) if k == "Feeds" && v.contains("state.n"))), "{:?}", pane(&app));
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(Tone::Head, t) if t.starts_with("Made of"))), "{:?}", pane(&app));

    // Grouped: each statement heads the nodes it is made of, down the page, the same statement
    // still picked; its frame's detail is the statement's.
    press(&mut app, KeyCode::Char('a'));
    let g = app.flow.graph.as_ref().unwrap();
    assert_eq!(g.level, Level::Grouped);
    assert_eq!(g.framed.iter().filter(|f| **f).count(), 3);
    let frames: Vec<String> = (0..g.rows.len()).filter(|i| g.is_frame(*i)).filter_map(|i| g.row(i)).map(|r| r.name.clone()).collect();
    assert_eq!(frames, ["n", "state.n", "return"]);
    // Every node is under the frame of its statement, which is on its line.
    let mut under = None;
    for i in 0..g.rows.len() {
        let r = g.row(i).unwrap();
        if g.is_frame(i) {
            under = Some(r.line);
        } else if !r.head {
            assert_eq!(Some(r.line), under, "{} under its statement", r.name);
        }
    }
    assert_eq!(g.rows.len(), 3 + g.graph.bands.iter().map(|b| b.rows.len()).sum::<usize>(), "every node once, and a frame a statement");
    assert!(g.is_frame(app.flow.sel()) && g.row(app.flow.sel()).map(|r| r.line) == Some(7), "the same statement picked");
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(Tone::Head, t) if t.starts_with("Made of"))), "{:?}", pane(&app));
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Grouped: ") && shown.contains("▾ state.n = n"), "{shown}");
    for (w, h) in [(80, 24), (50, 12)] {
        screen(&mut app, w, h);
    }

    // Every node a row again, the same statement still picked; enter edits the text there.
    press(&mut app, KeyCode::Char('a'));
    let g = app.flow.graph.as_ref().unwrap();
    assert!(g.level == Level::Nodes && g.rows.len() > 4);
    assert_eq!(g.row(app.flow.sel()).map(|r| r.line), Some(7));
    screen(&mut app, 130, 40);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Editor);
    assert_eq!(app.tabs[app.tab].buf.row, 6, "on line 7");
}

#[test]
fn the_inspector_reads_a_name_as_it_is_typed_with_its_grammar_and_an_example() {
    let (mut app, _) = app();
    tab_with(&mut app, BUMP);
    let hover = |app: &App| app.tabs[0].inspected.as_ref().map(|i| (i.word.clone(), i.hover.clone())).unwrap_or_default();
    assert!(screen(&mut app, 130, 40).contains("Inspector: import"), "the cursor on import, at the start");
    assert!(hover(&app).1.contains("names something the flow uses from outside a run"), "{:?}", hover(&app));
    // Typed, the name is read as it ends where the cursor is: what it is, and how it is written,
    // as its hover gives it in TypeScript's syntax.
    app.tabs[0].buf.go(4, 99);
    typed(&mut app, "\nexport const reset = action");
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Inspector: action"), "{shown}");
    let (word, said) = hover(&app);
    assert_eq!(word, "action");
    assert!(said.starts_with("`export const reset = action((input) => { … })`: a run by hand"), "{said}");
    // Its form, and a sample's lines that write it.
    let usage = app.tabs[0].inspected.as_ref().and_then(|i| i.usage.clone()).unwrap();
    assert_eq!(usage.forms[0].0, "export const NAME = action((input) => { … })");
    assert_eq!(usage.example.as_ref().map(|(s, l)| format!("{s}: {}", l[0])).as_deref(), Some("jwt-api: export const trust = action((input) => {"));
    assert!(shown.contains("from the sample jwt-api"), "{shown}");
    // The cursor elsewhere, on no name: the inspector keeps it.
    app.tabs[0].buf.go(6, 1);
    assert!(screen(&mut app, 130, 40).contains("Inspector: action"));
    // On another name, that one: `return`, on line 10 now.
    app.tabs[0].buf.go(10, 4);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Inspector: return"), "{shown}");
    assert!(hover(&app).1.starts_with("`return value`: what a route or an action answers"), "{:?}", hover(&app));
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
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.level == Level::Nodes), "every node at first");
    assert!(shown.contains(" a  nodes ") && !shown.lines().last().unwrap_or_default().contains("statements"), "a on the graph's edge, not the bar, the level shown in a narrow panel:\n{shown}");
    click(&mut app, Click::Every);
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.level == Level::Statements), "its switch: the statements");
    assert!(app.typing, "the text still has the keys");
    app.tabs[0].buf.go(8, 1);
    let shown = screen(&mut app, 130, 40);
    assert_eq!(picked(&app), "state.n", "the statement at the cursor");
    assert!(shown.contains("at the cursor") && shown.contains("flow(\"bump\")"), "{shown}");
    // A row clicked: the graph has the keys, ↑↓ pick, the text marks it and the cursor stays.
    click(&mut app, Click::Row(List::Graph, 1));
    assert!(app.beside_has_keys() && !app.typing, "a row clicked: the graph has the keys");
    assert_eq!((picked(&app), app.tabs[0].buf.row), ("n".to_string(), 7), "picked, the cursor where it was");
    press(&mut app, KeyCode::Down);
    // As wide again as the shell prompt's user@host, which the keys line gives room to.
    let shown = screen(&mut app, 130 + super::shell::who().chars().count() as u16, 40);
    assert_eq!(picked(&app), "state.n");
    assert!(shown.contains("· picking") && shown.contains("back to the text") && shown.contains("its detail"), "its keys on its edge:\n{shown}");
    let g = app.tabs[0].graph.as_ref().unwrap();
    let m = g.view.row(g.sel).unwrap().mark;
    assert_eq!((m.start.line, m.start.col, m.end.col), (8, 3, 10), "state.n, on line 8");
    press(&mut app, KeyCode::Char('a'));
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.level == Level::Grouped) && app.beside_has_keys(), "a: grouped, the graph keeping the keys");
    assert_eq!(picked(&app), "state.n", "its statement's frame");
    press(&mut app, KeyCode::Char('a'));
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.level == Level::Nodes), "a: every node");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    assert!(app.typing && !app.beside_has_keys() && app.tabs[0].buf.row == 7, "esc: the text has the keys, the cursor where it was");
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
    assert_eq!((app.tabs[0].buf.row, app.tabs[0].buf.col), (6, 8), "at n, line 7");
    screen(&mut app, 130, 40);
    // A second click on a row edits it there.
    app.graph_beside();
    screen(&mut app, 130, 40);
    click(&mut app, Click::Row(List::Graph, 2));
    click(&mut app, Click::Row(List::Graph, 2));
    assert!(app.typing && app.tabs[0].buf.row == 7, "state.n, line 8");
    // Typed, the graph is built again: a statement before the answer (none may come after it).
    app.tabs[0].buf.go(8, 99);
    typed(&mut app, "\nconst m = n + 1");
    screen(&mut app, 130, 40);
    assert!(app.tabs[0].graph.as_ref().unwrap().view.rows.len() == 5, "a statement more");
    assert_eq!(picked(&app), "m");
    // Its 15 characters, its indent and its line.
    for _ in 0..17 {
        press(&mut app, KeyCode::Backspace);
    }
    assert_eq!(app.tabs[0].buf.text(), BUMP);

    // The graph alone: its keys, at the statement the cursor was in.
    ctrl(&mut app, 'b');
    assert_eq!(app.show, Show::Graph);
    assert!(!app.typing, "its keys are the graph's");
    let g = app.tabs[0].graph.as_ref().unwrap();
    let names: Vec<String> = (0..g.view.rows.len()).filter_map(|i| g.view.row(i)).map(|r| r.name.clone()).collect();
    assert_eq!(names, ["action bump", "n", "state.n", "return"]);
    assert_eq!(picked(&app), "state.n");
    press(&mut app, KeyCode::Down);
    assert_eq!(picked(&app), "return");
    press(&mut app, KeyCode::Up);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("as typed") && shown.contains("state.n = n") && shown.contains("Takes from") && shown.contains('●'), "{shown}");
    press(&mut app, KeyCode::Char('a'));
    assert!(app.tabs[0].graph.as_ref().is_some_and(|g| g.view.level == Level::Grouped), "a: grouped");
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("Grouped: ") && shown.contains("▾ state.n = n"), "{shown}");
    assert_eq!(app.tabs[0].buf.text(), BUMP, "nothing typed into the text");
    screen(&mut app, 80, 24);

    // Enter: the text at the statement picked, the graph beside it.
    press(&mut app, KeyCode::Enter);
    assert!(app.show == Show::Both && app.typing);
    assert_eq!(app.tabs[0].buf.row, 7, "state.n = n, line 8");

    // The buttons on the tab bar pick one; esc leaves the graph for the code; a text that
    // doesn't compile says so.
    screen(&mut app, 130, 40);
    click(&mut app, Click::Show(Show::Graph));
    assert_eq!(app.show, Show::Graph);
    press(&mut app, KeyCode::Esc);
    assert!(app.show == Show::Code && app.typing);
    tab_with(&mut app, "import { flow, route } from \"holon\"\n\nflow(\"broken\")\n\nroute.get(\"/x\", (req) => {\n  return nope(\n})\n");
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
    // In the clock's body, after its write.
    app.tabs[0].buf.go(7, 99);
    typed(&mut app, "\nconst x = sta");
    let complete = app.complete.as_ref().expect("offered as a name is typed");
    assert!(complete.items.iter().any(|c| c.label == "state"), "{:?}", complete.items.iter().map(|c| &c.label).collect::<Vec<_>>());
    let at = complete.items.iter().position(|c| c.label == "state").unwrap();
    app.take_completion(Some(at));
    assert_eq!(app.tabs[0].buf.lines[7], "  const x = state");
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("const x = state"), "{shown}");

    ctrl(&mut app, 'z');
    assert_eq!(app.tabs[0].buf.lines[7], "  const x = sta", "undo takes back the completion");
    ctrl(&mut app, 'z');
    assert_eq!(app.tabs[0].buf.lines[7], "  const x =", "then a word at a time");
    for _ in 0..4 {
        ctrl(&mut app, 'z');
    }
    assert_eq!(app.tabs[0].buf.text(), BEATS, "and then the line");
    ctrl(&mut app, 'y');
    assert_ne!(app.tabs[0].buf.text(), BEATS);

    // Shift+arrows select; ctrl+c copies; ctrl+f finds and selects.
    app.tabs[0].buf.go(4, 1);
    app.key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));
    ctrl(&mut app, 'c');
    assert_eq!(app.clipboard, "flow(\"beats\")");
    assert_eq!(app.to_clipboard.as_deref(), Some("flow(\"beats\")"), "for the terminal's clipboard too");
    ctrl(&mut app, 'f');
    typed(&mut app, "every");
    assert_eq!(app.tabs[0].buf.selected().as_deref(), Some("every"));
    assert_eq!(app.tabs[0].buf.row, 5, "the next one after the cursor: the clock's, on line 6");
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
    // The Approvals page's runs waiting, as well as its deploys.
    app.page = Page::Approvals;
    app.approvals_part = super::approvals::Part::Runs;
    for (w, h) in [(80, 24), (140, 45), (40, 12)] {
        screen(&mut app, w, h);
    }
    app.approvals_part = super::approvals::Part::Deploys;
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
    let file = dir.join("fresh.holon.ts");
    let _ = std::fs::remove_file(&file);
    let mut app = App::new(None);
    press(&mut app, KeyCode::Char(':'));
    typed(&mut app, &format!("open {}\n", file.display()));
    assert_eq!(app.tabs.len(), 1, "{:?}", app.note.as_ref().map(|n| &n.text));
    let starter = "import { flow } from \"holon\"\n\nflow(\"fresh\")\n";
    assert_eq!(app.tabs[0].buf.text(), starter, "a new file starts as a flow named for it");
    assert!(app.tabs[0].dirty());
    ctrl(&mut app, 's');
    assert_eq!(std::fs::read_to_string(&file).unwrap(), starter);
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
fn an_input_line_is_edited_where_its_cursor_is() {
    let mut app = App::new(None);
    ctrl(&mut app, 'n');
    ctrl(&mut app, 's');
    let value = |app: &App| match &app.modal {
        Some(Modal::Input { value, at, .. }) => (value.clone(), *at),
        _ => panic!("no dialog"),
    };
    assert_eq!(value(&app), ("my-flow.holon.ts".into(), 16), "the cursor at its end");
    // Back over ".holon.ts", a word typed there, a letter deleted on either side.
    for _ in 0..9 {
        press(&mut app, KeyCode::Left);
    }
    for c in "-two".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(value(&app), ("my-flow-two.holon.ts".into(), 11));
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Backspace);
    assert_eq!(value(&app), ("my-flow-twholon.ts".into(), 10));
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(value(&app), ("amy-flow-twholon.tsx".into(), 20));
    // A click puts the cursor at the character clicked.
    screen(&mut app, 120, 30);
    let (rect, _) = *app.hits.borrow().iter().rev().find(|(_, c)| matches!(c, Click::Input { .. })).expect("the line wasn't drawn");
    app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 3, row: rect.y, modifiers: KeyModifiers::NONE });
    assert_eq!(value(&app).1, 3);
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("› amy-flow-twholon.tsx"), "{shown}");
}

/// Every line typed outside a dialog of its own (`:`, `/`, the palette, find, the shell line,
/// the call dialog's field) edits the same way, key by key: the cursor goes back, typing goes
/// in there, delete and backspace take a character on either side, and a click places it.
#[test]
fn every_line_typed_edits_where_its_cursor_is() {
    type Open = fn() -> App;
    let lines: [(&str, Open); 6] = [
        (":", || {
            let mut app = App::new(None);
            press(&mut app, KeyCode::Char(':'));
            app
        }),
        ("/", || {
            let mut app = App::new(None);
            app.go(Page::Flows);
            press(&mut app, KeyCode::Char('/'));
            app
        }),
        ("palette", || {
            let mut app = App::new(None);
            ctrl(&mut app, 'p');
            app
        }),
        ("find", || {
            let mut app = App::new(None);
            tab_with(&mut app, BUMP);
            ctrl(&mut app, 'f');
            app
        }),
        ("shell", || {
            let mut app = App::new(None);
            ctrl(&mut app, '`');
            app
        }),
        ("call", || {
            let (mut app, _) = bumped(0);
            press(&mut app, KeyCode::Esc);
            app.call("bump");
            press(&mut app, KeyCode::Tab);
            app
        }),
    ];
    for (name, open) in lines {
        let mut app = open();
        assert!(app.field().is_some(), "{name}: no line open");
        let text = |app: &App| app.field_text().unwrap_or_else(|| panic!("{name}: the line closed")).to_string();
        // What it starts with (the call's `{}`) taken away first.
        for _ in 0..text(&app).chars().count() {
            press(&mut app, KeyCode::Backspace);
        }
        typed(&mut app, "abcd");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(text(&app), "abXcd", "{name}: typed where the cursor went back to");
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Right);
        assert_eq!(text(&app), "bXc", "{name}: delete took the first, backspace the last");
        let shown = screen(&mut app, 120, 30);
        assert!(shown.contains("bXc"), "{name}: {shown}");
        let (rect, _) = *app.hits.borrow().iter().rev().find(|(_, c)| matches!(c, Click::Input { .. })).unwrap_or_else(|| panic!("{name}: the line takes no click"));
        app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        press(&mut app, KeyCode::Char('Z'));
        assert_eq!(text(&app), "bZXc", "{name}: the click put the cursor after the first character");
        app.paste("pq");
        assert_eq!(text(&app), "bZpqXc", "{name}: pasted at the cursor");
    }
}

/// A line longer than its room scrolls to keep the cursor in view.
#[test]
fn a_long_line_scrolls_to_its_cursor() {
    let mut app = App::new(None);
    press(&mut app, KeyCode::Char(':'));
    let long: String = (0..200).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    app.paste(&format!("{long}END"));
    assert!(screen(&mut app, 80, 24).contains("END"), "at the end, the end shows");
    press(&mut app, KeyCode::Home);
    let shown = screen(&mut app, 80, 24);
    assert!(shown.contains(":abcdef") && !shown.contains("END"), "{shown}");
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
    let there = dir.join("there.holon.ts");
    let kept = "import { flow } from \"holon\"\n\nflow(\"there\")\n";
    std::fs::write(&there, kept).unwrap();
    answer(&mut app, &there.display().to_string());
    assert!(app.note.as_ref().is_some_and(|n| n.error && n.text.contains("there already")));
    assert_eq!(std::fs::read_to_string(&there).unwrap(), kept);
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
        text: ["import { flow, route } from \"holon\"", "", "flow(\"nearest\")", "", "route.get(\"/nearest/:word\", (req) => {", "  const best = closest(req.params.word)", "  return best", "})"].map(String::from).to_vec(),
        notes: [(6, vec!["⇒ nothing: closest gave nothing: out of fuel".to_string()]), (7, vec!["⇒ nothing: closest gave nothing: out of fuel".to_string()])].into_iter().collect(),
        after: vec!["answered 500: ".into(), "failed: closest gave nothing".into()],
        steps: vec![],
    };
    let entry = serde_json::json!({ "seq": 3, "kind": "request", "method": "GET", "url": "/nearest/kittne", "status": 500, "duration_ms": 2.5, "error": "closest gave nothing" });
    let mut runs = Runs { slug: "nearest".into(), entries: vec![entry], sel: 0, view: Some(Ok(view)), step: 0, pane: RunPane::Details };
    assert_eq!(runs.errors(), [(None, "closest gave nothing".to_string()), (Some(6), "closest gave nothing: out of fuel".to_string())]);
    assert_eq!(runs.panes(), [RunPane::Details, RunPane::Errors, RunPane::Timeline]);
    runs.pane = RunPane::Errors;
    app.tabs[0].runs = Some(runs);
    app.typing = false;
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains(" Errors (2) ") && shown.contains("line 6") && shown.contains("const best = closest(req.params.word)") && shown.contains("out of fuel"), "{shown}");
    assert!(shown.contains("GET /nearest/kittne") && shown.contains("err"), "the run's row says it failed: {shown}");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tabs[0].runs.as_ref().unwrap().pane, RunPane::Timeline);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("It ran nothing this text says.") || shown.contains("recorded no timing"), "{shown}");
    for (w, h) in [(80, 24), (60, 16), (40, 12)] {
        screen(&mut app, w, h);
    }
}

/// The calls an editor made, recorded.
type Calls = std::sync::Arc<crate::sync::Mutex<Vec<String>>>;

/// A server with accounts and two runs of `payouts` waiting at a two-person approval: ada and bob
/// admins, wes a viewer; the editor as each, by name, each one's calls recorded.
fn waiting_payouts() -> (&'static AppState, impl Fn(&str) -> (App, Calls)) {
    let mut state = test_state();
    let text = "import { action, approval, flow } from \"holon\"

flow(\"payouts\")

export const pay = action(async (order) => {
  const decision = await approval(\"payout\", { expires: \"1h\", people: 2 })
  return decision.approved
})
";
    let unit = crate::tests::stage_text(&state, text);
    let ack = serde_json::json!({ "acknowledge": ["Action(\"pay\")", "Approval(\"payout/person*2\")"] });
    assert_eq!(crate::tests::deploy_json(&state, "payouts", unit, ack).status_code, 200);
    for amount in [10, 20] {
        assert!(crate::tests::run_action_named(&state, "payouts", "pay", serde_json::json!({ "amount": amount })).is_success());
    }
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let pw = "correct horse battery";
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", pw).unwrap();
    let bob = book.accept(&book.invite("bob", "admin", None, "ada").unwrap(), pw).unwrap().0;
    let wes = book.accept(&book.invite("wes", "viewer", None, "ada").unwrap(), pw).unwrap().0;
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    let state: &'static AppState = Box::leak(Box::new(state));
    let tokens = [("ada", ada), ("bob", bob), ("wes", wes)];
    let open = move |who: &str| {
        let token = tokens.iter().find(|(n, _)| *n == who).unwrap().1.clone();
        let calls = std::sync::Arc::new(crate::sync::Mutex::new(vec![]));
        let mut app = App::new(None);
        app.attach(Client::new(Box::new(Recorded(InProcess(state, Some(token)), std::sync::Arc::clone(&calls)))));
        app.refresh();
        app.go(Page::Approvals);
        press(&mut app, KeyCode::Tab);
        (app, calls)
    };
    (state, open)
}

fn note(app: &App) -> String {
    app.note.as_ref().map(|n| n.text.clone()).unwrap_or_default()
}

/// Runs waiting at an approval are decided on the Approvals page, as the web console's Runs waiting
/// decides them: ada approves one of two, and it waits for one more person; she can't approve it
/// twice; bob's approval lets it go on. A decline is never sent without why, and with it ends the
/// other run declined. wes, a viewer, sees them and can take no step, and sends nothing.
#[test]
fn the_approvals_page_decides_runs_waiting_at_an_approval() {
    let (_, open) = waiting_payouts();
    let (mut ada, _) = open("ada");
    assert_eq!(ada.runs_waiting().len(), 2);
    assert_eq!(ada.runs_yours(), 2, "both are hers to decide");
    let shown = screen(&mut ada, 140, 40);
    assert!(shown.contains("Runs waiting 2") && shown.contains("payouts") && shown.contains("payout") && shown.contains("a person 0/2"), "{shown}");
    assert!(shown.contains("4 waiting for you") || shown.contains("2 waiting for you"), "the top bar counts them: {shown}");
    let first = ada.runs_waiting()[0]["run_id"].as_str().unwrap().to_string();
    let second = ada.runs_waiting()[1]["run_id"].as_str().unwrap().to_string();

    // A decline without why stays open, saying so; nothing is sent.
    press(&mut ada, KeyCode::Char('d'));
    assert!(matches!(ada.modal, Some(Modal::Input { .. })));
    press(&mut ada, KeyCode::Enter);
    assert!(matches!(ada.modal, Some(Modal::Input { .. })), "still asks why");
    assert!(note(&ada).contains("says why"), "{}", note(&ada));
    press(&mut ada, KeyCode::Esc);

    // ada approves the first: one of two.
    press(&mut ada, KeyCode::Char('a'));
    let Some(Modal::Input { title, lines, .. }) = &ada.modal else { panic!("no dialog: {}", note(&ada)) };
    assert!(title.contains("Approve \"payout\"") && title.contains("payouts"), "{title}");
    assert!(lines.iter().any(|l| l.contains("1 more person")), "{lines:?}");
    typed(&mut ada, "fine by me
");
    assert!(note(&ada).contains("1 of 2") && note(&ada).contains("1 more person"), "{}", note(&ada));
    // Not twice: the run is still there, and none of its steps is hers now.
    let again = ada.runs_waiting().iter().position(|r| r["run_id"] == first.as_str()).unwrap();
    ada.run_sel = again;
    press(&mut ada, KeyCode::Char('a'));
    assert!(ada.modal.is_none(), "not hers twice");

    // bob approves it: it goes on.
    let (mut bob, _) = open("bob");
    bob.run_sel = bob.runs_waiting().iter().position(|r| r["run_id"] == first.as_str()).unwrap();
    press(&mut bob, KeyCode::Char('a'));
    press(&mut bob, KeyCode::Enter);
    assert!(note(&bob).contains("goes on"), "{}", note(&bob));
    assert!(!bob.runs_waiting().iter().any(|r| r["run_id"] == first.as_str()), "it no longer waits");

    // bob declines the other, saying why: it ends declined.
    bob.run_sel = bob.runs_waiting().iter().position(|r| r["run_id"] == second.as_str()).unwrap();
    press(&mut bob, KeyCode::Char('d'));
    typed(&mut bob, "too much at once
");
    assert!(note(&bob).contains("Declined"), "{}", note(&bob));
    assert!(bob.runs_waiting().is_empty());

    // wes sees what waits, and takes no step.
    let (_, open) = waiting_payouts();
    let (mut wes, calls) = open("wes");
    assert_eq!(wes.runs_yours(), 0);
    let shown = screen(&mut wes, 140, 40);
    assert!(shown.contains("None of its steps is yours"), "{shown}");
    press(&mut wes, KeyCode::Char('a'));
    assert!(wes.modal.is_none());
    assert!(!calls.lock().unwrap().iter().any(|c| c.contains("/decide")), "nothing sent");
    for (w, h) in [(80, 24), (60, 16)] {
        screen(&mut wes, w, h);
    }
}

/// The admin page shows each part to whoever may see it, and a key whoever may not use it says
/// why and sends nothing: wes, whose role holds people.view alone, reads the people, can't invite
/// anyone, and is told why the folders aren't shown.
#[test]
fn the_admin_page_shows_each_part_to_whoever_may_see_it() {
    let mut state = test_state();
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let pw = "correct horse battery";
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", pw).unwrap();
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    book.define_role(crate::bindings::RoleText { name: "lookout".into(), about: None, allows: vec!["people.view".into()], kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let wes = book.accept(&book.invite("wes", "lookout", None, "ada").unwrap(), pw).unwrap().0;
    let state: &'static AppState = Box::leak(Box::new(state));
    let calls = std::sync::Arc::new(crate::sync::Mutex::new(vec![]));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(Recorded(InProcess(state, Some(wes)), std::sync::Arc::clone(&calls)))));
    app.refresh();
    press(&mut app, KeyCode::Char('7'));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("ada") && shown.contains("wes"), "the people: {shown}");
    let before = calls.lock().unwrap().len();
    press(&mut app, KeyCode::Char('n'));
    assert!(app.modal.is_none(), "no invite dialog for wes");
    let note = app.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(note.contains("people.manage"), "says what it takes: {note}");
    assert_eq!(calls.lock().unwrap().len(), before, "sends nothing");
    app.admin_part(Part::Folders);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("It can't be shown here") && shown.contains("resource.view"), "folders: {shown}");
    assert!(!calls.lock().unwrap().iter().any(|c| c.starts_with("GET /admin/folders")), "the folders aren't read for wes");
    // ada, an admin, sees every part as before.
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, Some(ada)))));
    app.refresh();
    press(&mut app, KeyCode::Char('7'));
    app.admin_part(Part::Folders);
    let shown = screen(&mut app, 140, 40);
    assert!(!shown.contains("It can't be shown here"), "{shown}");
}

#[test]
fn the_admin_page_invites_people_makes_keys_and_binds_folders_and_databases() {
    let (mut app, _) = app();
    press(&mut app, KeyCode::Char('7'));
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("It can't be shown here") && shown.contains("no accounts"), "{shown}");

    let mut app = admin_app();
    press(&mut app, KeyCode::Char('7'));
    assert_eq!(app.page, Page::Admin);
    // Each part's tab shows when it is picked, the bar scrolled to it if need be.
    for (part, name) in super::admin::PARTS {
        app.admin_part(part);
        let shown = screen(&mut app, 140, 40);
        assert!(shown.lines().nth(1).is_some_and(|bar| bar.contains(name)), "{name}: {shown}");
    }
    app.admin_part(Part::People);

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
    // A service for a script, owned by ana; a person's email isn't asked of it.
    press(&mut app, KeyCode::Char('a'));
    answer(&mut app, "nightly service viewer ana");
    let nightly = app.admin.users.iter().find(|u| u["name"] == "nightly").unwrap_or_else(|| panic!("{:?}", app.note.as_ref().map(|n| &n.text)));
    assert_eq!((&nightly["kind"], &nightly["owner"]), (&serde_json::json!("service"), &serde_json::json!("ana")));
    let at = app.admin.users.iter().position(|u| u["name"] == "nightly").unwrap();
    app.admin_pick(List::Admin, at, false);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("a service") && shown.contains("Owner"), "{shown}");

    // API keys: made for the service, shown once, then revoked.
    app.admin_part(Part::Keys);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "nightly the nightly report");
    assert!(matches!(&app.modal, Some(Modal::Lines { title, .. }) if title == "nightly: an API key"), "{:?}", app.note.as_ref().map(|n| &n.text));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.admin.tokens.len(), 1);
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Enter);
    assert!(app.admin.tokens.is_empty(), "{:?}", app.note.as_ref().map(|n| &n.text));

    // Folders: bound with holon folder add's flags, then unbound.
    let dir = std::env::temp_dir().join(format!("holon-admin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    app.admin_part(Part::Folders);
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
    app.admin_part(Part::Databases);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, &format!("shop {} --schema create table if not exists items (id integer primary key)", dir.join("shop.db").display()));
    assert_eq!(app.admin.databases.len(), 1, "{:?}", app.note.as_ref().map(|n| &n.text));
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Lines { lines, .. }) = &app.modal else { panic!("no tables: {:?}", app.note.as_ref().map(|n| &n.text)) };
    assert!(lines.iter().any(|(_, l)| l == "items"), "{lines:?}");
    press(&mut app, KeyCode::Esc);

    // Secrets and the server: their panes read the server.
    app.admin_part(Part::Secrets);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Read from the environment"), "{shown}");
    app.admin_part(Part::Server);
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
    press(&mut app, KeyCode::Char('t'));
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
    let text = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"peek\")\n\nexport const peek = action((input) => state.n ?? 0, { log: \"unrecorded\" })\n\nexport const bump = action((input) => {\n  state.n = (state.n ?? 0) + 1\n  return \"bumped\"\n})\n";
    tab_with(&mut app, text);
    ctrl(&mut app, 'd');
    let Some(Modal::Approve { .. }) = &app.modal else { panic!("no dialog: {:?}", app.note.as_ref().map(|n| &n.text)) };
    let shown = screen(&mut app, 140, 44);
    assert!(shown.contains("+ log action peek: not recorded"), "the approval says it: {shown}");
    press(&mut app, KeyCode::Char('t'));
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

/// A flow's data and its calls outside have a colour each, wherever they show: a statement is
/// what its nodes are (a call outside first, then its data, then its answer).
#[test]
fn statements_are_coloured_by_what_they_touch() {
    use crate::workbench::graph::Kind;
    assert_eq!(Kind::of_leaf("db_query"), Kind::Data);
    assert_eq!(Kind::of_leaf("collection_put"), Kind::Data);
    assert_eq!(Kind::of_leaf("http_request"), Kind::Outside);
    assert_eq!(Kind::of_leaf("respond"), Kind::Answer);
    assert_eq!(Kind::of_leaf("add"), Kind::Compute);
    let text = holon_author::samples::sample("cache-aside").unwrap().text;
    let kinds = Kind::by_line(text);
    let line = |s: &str| text.lines().position(|l| l.contains(s)).unwrap() as u32 + 1;
    assert_eq!(kinds.get(&line("const id = ids[name] ??")), Some(&Kind::Outside), "the fetch: {kinds:?}");
    assert_eq!(kinds.get(&line("ids[name] = id")), Some(&Kind::Data), "the write: {kinds:?}");
}

/// The text's title says how it stands against what is live, and the tab its deployment's state.
#[test]
fn the_text_says_how_it_differs_from_live() {
    let (mut app, _) = app();
    tab_with(&mut app, BUMP);
    assert!(screen(&mut app, 130, 30).contains("not deployed"), "{}", screen(&mut app, 130, 30));
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    let shown = screen(&mut app, 130, 30);
    assert!(shown.contains("v1, live"), "{shown}");
    assert!(shown.contains("● bump"), "the tab's dot: {shown}");
    app.tabs[app.tab].buf = super::text::Buffer::new(&BUMP.replace("+ 1", "+ 2"));
    app.tabs[app.tab].check = super::app::Check::of(&app.tabs[app.tab].buf.text());
    let shown = screen(&mut app, 130, 30);
    assert!(shown.contains("changed from v1 *"), "{shown}");
}

/// A deploy's dialog can save it on the server instead, for another to approve.
#[test]
fn a_deploy_can_be_saved_for_another_to_approve() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    ctrl(&mut app, 'd');
    assert!(screen(&mut app, 130, 40).contains("save, don't deploy"), "{}", screen(&mut app, 130, 40));
    press(&mut app, KeyCode::Char('s'));
    assert!(app.modal.is_none());
    let c = app.client.as_ref().unwrap();
    assert!(c.deployment("beats").is_none(), "not deployed");
    assert!(app.waiting().iter().any(|(slug, _)| slug == "beats"), "it waits: {:?}", app.waiting());
}

/// Over runs, , and . go to the run before and after, and the line under the code picks a step.
#[test]
fn runs_go_back_and_on_with_comma_and_period() {
    let (mut app, _) = bumped(3);
    ctrl(&mut app, 'r');
    assert_eq!(app.tabs[app.tab].runs.as_ref().unwrap().sel, 0);
    press(&mut app, KeyCode::Char(','));
    assert_eq!(app.tabs[app.tab].runs.as_ref().unwrap().sel, 1);
    press(&mut app, KeyCode::Char('.'));
    assert_eq!(app.tabs[app.tab].runs.as_ref().unwrap().sel, 0);
    let shown = screen(&mut app, 130, 40);
    assert!(shown.contains("●──") || shown.contains("━●"), "the scrubber: {shown}");
}

/// The lists say more in each row: the Overview's columns and a version waiting, a run's
/// writes, a sample's tags and its shell line.
#[test]
fn the_lists_say_more_in_a_row() {
    let (mut app, _) = bumped(2);
    let plan = holon_author::agent::plan(&BUMP.replace("+ 1", "+ 2")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    app.client.as_ref().unwrap().save("bump", &plan, &BUMP.replace("+ 1", "+ 2"), None).unwrap();
    app.refresh();
    app.go(Page::Overview);
    let shown = screen(&mut app, 150, 36);
    assert!(shown.contains("name") && shown.contains("last run"), "the columns: {shown}");
    assert!(shown.contains("v2 waits for you"), "{shown}");
    app.open_flow("bump");
    app.flow_section(Section::Runs);
    let shown = screen(&mut app, 150, 36);
    assert!(shown.contains("✎1"), "what it wrote: {shown}");
    app.go(Page::Library);
    let shown = screen(&mut app, 150, 36);
    assert!(shown.contains("holon new hello-api.holon.ts --sample hello-api"), "{shown}");
}

/// A tab opened from the server saves over the save it was opened from, and over its own saves
/// since, but not over one made since by someone else (R32): theirs is refused, not lost, and
/// stands.
#[test]
fn a_tab_saves_over_the_save_it_opened_and_not_over_a_save_made_since() {
    let (mut app, _) = app();
    let plan = holon_author::agent::plan(BEATS).unwrap();
    app.client.as_ref().unwrap().save("beats", &plan, BEATS, None).unwrap();
    app.refresh();
    app.edit("beats");
    let i = app.tab;
    assert_eq!(app.tabs[i].slug.as_deref(), Some("beats"));
    for n in 1..=2 {
        app.save(i, None);
        let note = app.note.as_ref().unwrap();
        assert!(!note.error, "save {n}: {}", note.text);
    }
    let theirs = format!("// theirs\n{BEATS}");
    app.client.as_ref().unwrap().save("beats", &plan, &theirs, None).unwrap();
    app.save(i, None);
    let note = app.note.as_ref().unwrap();
    assert!(note.error && note.text.contains("was saved again"), "{}", note.text);
    assert_eq!(app.client.as_ref().unwrap().saved_text("beats").unwrap().0, theirs, "theirs stands");

    // Saved by someone else after it was opened and before it is first saved here: refused too.
    let bump = holon_author::agent::plan(BUMP).unwrap();
    app.client.as_ref().unwrap().save("bump", &bump, BUMP, None).unwrap();
    app.refresh();
    app.edit("bump");
    let j = app.tab;
    let theirs = format!("// theirs\n{BUMP}");
    app.client.as_ref().unwrap().save("bump", &bump, &theirs, None).unwrap();
    app.save(j, None);
    let note = app.note.as_ref().unwrap();
    assert!(note.error && note.text.contains("was saved again"), "{}", note.text);
    assert_eq!(app.client.as_ref().unwrap().saved_text("bump").unwrap().0, theirs, "theirs stands");
}

/// `/` narrows the flows and the library in place, `f` goes round the flows' chips, and esc
/// clears the filter.
#[test]
fn slash_filters_a_list_in_place() {
    let (mut app, _) = bumped(0);
    let plan = holon_author::agent::plan(BEATS).unwrap();
    app.client.as_ref().unwrap().save("beats", &plan, BEATS, None).unwrap();
    app.refresh();
    app.go(Page::Flows);
    assert_eq!(app.flow_names(), ["beats", "bump"]);
    press(&mut app, KeyCode::Char('/'));
    for c in "bea".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(app.flow_names(), ["beats"]);
    let shown = screen(&mut app, 130, 30);
    assert!(shown.contains("/bea") && shown.contains("Flows (1 of 2)"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert!(!app.filtering);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.flow_names().len(), 2, "esc clears it");
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(app.flow_names(), ["bump"], "live: deployed and running");
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(app.flow_names(), ["beats"], "waiting: saved, never deployed");
    app.go(Page::Library);
    press(&mut app, KeyCode::Char('/'));
    for c in "cache".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_eq!(app.library().iter().map(|s| s.name).collect::<Vec<_>>(), ["cache-aside"]);
}

/// The `:` line suggests what it may become as it is typed; tab takes the first.
#[test]
fn the_command_line_suggests() {
    let (mut app, _) = bumped(0);
    app.go(Page::Overview);
    press(&mut app, KeyCode::Char(':'));
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(app.suggestions().first().map(|(l, _)| l.as_str()), Some("pause "));
    assert!(screen(&mut app, 130, 30).contains(":pause"), "{}", screen(&mut app, 130, 30));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(app.suggestions().first().map(|(l, _)| l.as_str()), Some("pause bump"));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.prompt.as_deref(), Some("pause bump"));
}

/// Approvals reviews the wait picked beside the list, as its deploy would: enter approves it
/// there, ] goes to the next.
#[test]
fn approvals_review_beside_the_list() {
    let (mut app, _) = app();
    for text in [BEATS, BUMP] {
        let plan = holon_author::agent::plan(text).unwrap();
        app.client.as_ref().unwrap().save(&plan.name, &plan, text, None).unwrap();
    }
    app.refresh();
    app.go(Page::Approvals);
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("beats  ·  1 of 2"), "{shown}");
    assert!(shown.contains("If approved, while it runs it may") && shown.contains("+ "), "{shown}");
    press(&mut app, KeyCode::Char(']'));
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("bump  ·  2 of 2"), "{shown}");
    press(&mut app, KeyCode::Right);
    assert!(app.approval.as_ref().is_some_and(|a| a.keys), "the review has the keys");
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert!(app.client.as_ref().unwrap().deployment("bump").is_some(), "{:?}", app.note.as_ref().map(|n| &n.text));
    let shown = screen(&mut app, 150, 40);
    assert!(shown.contains("beats  ·  1 of 1"), "{shown}");
}

/// The editor's Home tab: a draft, a sample, the tabs closed lately (enter opens one again, as
/// it was), the server's flows.
#[test]
fn home_opens_again_what_was_closed() {
    let (mut app, _) = bumped(0);
    let shown = screen(&mut app, 130, 30);
    assert!(shown.contains("⌂"), "{shown}");
    app.typing = false;
    press(&mut app, KeyCode::Char('w'));
    assert!(app.tabs.is_empty());
    let shown = screen(&mut app, 130, 30);
    assert!(shown.contains("⌂ Home") && shown.contains("a new draft") && shown.contains("closed"), "{shown}");
    assert_eq!(app.home_rows()[2], super::app::HomeRow::Closed(0));
    app.home_sel = 2;
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.tabs.len(), 1, "opened again");
    assert!(!app.at_home());
    screen(&mut app, 130, 30);
    click(&mut app, Click::Home);
    assert!(app.at_home());
    press(&mut app, KeyCode::Esc);
    assert!(!app.at_home(), "esc goes back to the tab");
}

/// A run's detail says where it is on record (its hash, the one before), how to check it
/// offline; `/` filters the runs, i explains one, F follows the newest.
#[test]
fn a_runs_record_its_filter_and_its_explanation() {
    let (mut app, _) = bumped(3);
    app.open_flow("bump");
    app.flow_section(Section::Runs);
    let d = app.detail.as_ref().unwrap();
    let (hash, prev, follows) = d.on_record(&d.runs[1]);
    assert_eq!(hash.as_deref(), d.runs[2]["prev"].as_str(), "a run's hash is what the next names");
    assert_eq!(prev, d.runs[1]["prev"].as_str().unwrap_or_default());
    assert!(follows);
    assert_eq!(d.on_record(d.runs.last().unwrap()).0.as_deref(), Some(d.head.as_str()), "the newest's is the head");
    assert!(pane(&app).iter().any(|r| matches!(r, Row::Text(_, t) if t.contains("holon verify bump-runs.json"))), "{:?}", pane(&app));
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('#'));
    press(&mut app, KeyCode::Char('1'));
    press(&mut app, KeyCode::Enter);
    let shown = screen(&mut app, 150, 36);
    assert!(shown.contains("Runs, newest first (1 of 3)  /#1"), "{shown}");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.flow_rows(), 3, "esc clears it");
    press(&mut app, KeyCode::Char('i'));
    assert!(matches!(&app.modal, Some(Modal::Lines { title, .. }) if title.contains("explained")), "{:?}", app.note.as_ref().map(|n| &n.text));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('F'));
    assert!(app.flow.follow);
}

/// The sidebar says whether the run logs' chains hold; `:theme light` draws for a light
/// terminal: no ground left dark.
#[test]
fn the_logs_health_and_a_light_theme() {
    let (mut app, _) = bumped(2);
    app.go(Page::Overview);
    let shown = screen(&mut app, 150, 44);
    let record = |shown: &str| shown.lines().find(|l| l.contains("on record") && !l.contains("of them")).map(|l| l.chars().take(21).collect::<String>());
    assert!(shown.contains("run logs intact"), "{shown}");
    assert_eq!(record(&shown).as_deref(), Some("    2 on record     │"), "{shown}");
    // However many, the count keeps inside the sidebar.
    app.client.as_mut().unwrap().deployments[0]["trigger_count"] = serde_json::json!(1_234_567);
    let shown = screen(&mut app, 150, 44);
    assert_eq!(record(&shown).as_deref(), Some("    1.2M on record  │"), "{shown}");
    assert!(shown.contains("Runs, last 24 h"), "{shown}");
    app.run_command("theme light");
    assert!(app.light);
    let mut terminal = Terminal::new(TestBackend::new(150, 44)).unwrap();
    terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
    let dark = terminal.backend().buffer().content.iter().filter(|c| matches!(c.bg, ratatui::style::Color::Indexed(232..=241))).count();
    assert_eq!(dark, 0, "no dark ground");
}

/// Every page with something on it, by name: for drawing them at different widths.
fn every_page() -> Vec<(String, App)> {
    let mut out = vec![];
    let rich = || {
        let (mut app, _) = bumped(3);
        tab_with(&mut app, BEATS);
        ctrl(&mut app, 'd');
        press(&mut app, KeyCode::Char('s'));
        app.refresh();
        press(&mut app, KeyCode::Esc);
        app
    };
    for page in [Page::Overview, Page::Approvals, Page::Flows, Page::Library] {
        let mut app = rich();
        app.go(page);
        out.push((format!("{page:?}"), app));
    }
    for (s, name) in super::flow::SECTIONS {
        let mut app = rich();
        app.open_flow("bump");
        app.flow_section(s);
        out.push((format!("Flow {name}"), app));
    }
    let mut app = rich();
    app.go(Page::Editor);
    app.tab = 0;
    out.push(("Editor text".into(), app));
    let mut app = rich();
    app.go(Page::Editor);
    app.tab = 0;
    ctrl(&mut app, 'r');
    out.push(("Editor runs".into(), app));
    let mut app = rich();
    app.go(Page::Editor);
    app.go_home();
    out.push(("Editor home".into(), app));
    let mut app = rich();
    app.go(Page::Editor);
    app.tab = 0;
    ctrl(&mut app, 'd');
    out.push(("Deploy dialog".into(), app));
    for (part, name) in super::admin::PARTS {
        let mut app = admin_app();
        app.go(Page::Admin);
        app.admin_part(part);
        out.push((format!("Admin {name}"), app));
    }
    out
}

#[test]
#[ignore = "writes every page at three widths to HOLON_SCREENS, for a person to read"]
fn dump_every_page() {
    let to = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write them");
    let mut all = String::new();
    for (name, mut app) in every_page() {
        for (w, h) in [(80, 24), (100, 30), (140, 40)] {
            all += &format!("\n===== {name} at {w}x{h} =====\n{}\n", screen(&mut app, w, h));
        }
    }
    std::fs::write(to, all).unwrap();
}

/// Every page draws at a narrow, a middling and a wide terminal (and a cramped one), the
/// server's name whole on top and where you are after it, the page you are on at least.
#[test]
fn every_page_draws_at_every_width() {
    for (name, mut app) in every_page() {
        for (w, h) in [(60, 20), (80, 24), (100, 30), (140, 40)] {
            let shown = screen(&mut app, w, h);
            let lines: Vec<&str> = shown.lines().collect();
            assert!(lines[0].contains("the test server"), "{name} at {w}x{h}: the server's name, whole:\n{shown}");
            let page = super::ui::crumb_last(&app);
            assert!(lines[0].contains("the test server ›") && lines[0].contains(&format!("› {page}")), "{name} at {w}x{h}: where you are, on top:\n{shown}");
        }
    }
}


/// Where you are is said on top, after the server's name, and a crumb before the last goes
/// back to it: the flow's name to its status, Flows to the list.
#[test]
fn the_crumbs_on_top_go_back() {
    let (mut app, _) = bumped(1);
    app.open_flow("bump");
    app.flow_section(Section::Runs);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.lines().next().unwrap().contains("the test server › Flows › bump › Runs"), "{shown}");
    let on_top = |app: &App, what: Click| app.hits.borrow().iter().find(|(r, c)| r.y == 0 && *c == what).map(|(r, _)| *r).unwrap_or_else(|| panic!("{what:?} isn't on top"));
    let down = |app: &mut App, r: ratatui::layout::Rect| app.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x, row: r.y, modifiers: KeyModifiers::NONE });
    let r = on_top(&app, Click::Section(Section::Status));
    down(&mut app, r);
    assert_eq!((app.page, app.flow.section), (Page::Flow, Section::Status));
    screen(&mut app, 140, 40);
    let r = on_top(&app, Click::Page(Page::Flows));
    down(&mut app, r);
    assert_eq!(app.page, Page::Flows);
}

/// The editor as `token`, its calls recorded as `METHOD /path?query`.
struct Recorded(InProcess, std::sync::Arc<crate::sync::Mutex<Vec<String>>>);

impl Api for Recorded {
    fn server(&self) -> String {
        self.0.server()
    }
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
        self.1.lock().unwrap().push(format!("{method} {path}"));
        self.0.send(method, path, body)
    }
}

/// The terminal editor shows what the server keeps only to whoever holds its grant (the audit
/// register's R60, beside tests/surfaces.rs's sweep of the commands and tools): a run's values to
/// run.steps, a flow's state to state.view, its log lines to run.view and an agent only where a
/// person let agents read them. ada, an admin, sees each canary; wes, a person holding a viewer's
/// actions but run.steps and state.view, none but the log line; scout, an agent (its role holds
/// state.view, not run.steps), the kept one alone. Each is shown every run's every tab, a flow's
/// state, history and log lines. The routes the editor calls are those surfaces.rs's row of
/// `editor` names.
#[test]
fn the_editor_shows_what_the_server_keeps_only_to_its_grants() {
    const CANARY: &str = "live_canary_7f3a";
    const KEPT: &str = "kept_canary_4d2b";
    const LOG: &str = "log_canary_9e1c";
    let mut state = test_state();
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let pw = "correct horse battery";
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", pw).unwrap();
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    let glance = ["flow.view", "run.view", "run.export", "secret.list", "policy.view"].map(String::from).to_vec();
    book.define_role(crate::bindings::RoleText { name: "glance".into(), about: None, allows: glance, kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let wes = book.accept(&book.invite("wes", "glance", None, "ada").unwrap(), pw).unwrap().0;
    let state: &'static AppState = Box::leak(Box::new(state));
    let as_ada = InProcess(state, Some(ada.clone()));
    let send = |method: &str, path: &str, body: serde_json::Value| crate::mcp::json_of(&as_ada, method, path, Some(Body::Json(body))).unwrap_or_else(|e| panic!("{method} {path}: {e}"));
    send("POST", "/people", serde_json::json!({ "name": "scout", "kind": "agent", "role": "agent" }));
    let scout = send("POST", "/people/scout/tokens", serde_json::json!({ "label": "its editor" }))["token"].as_str().unwrap().to_string();
    let flows = [
        ("steps", "import { action, approval, flow, route } from \"holon\"\n\nflow(\"steps\", { port: 18291 })\n\nroute.post(\"/check\", (req) => {\n  const key = req.json.key\n  return key.startsWith(\"live_\") ? \"a live key\" : \"a test key\"\n})\n\nexport const pay = action(async (order) => {\n  const decision = await approval(\"payout\", { expires: \"1h\" })\n  return decision.approved\n})\n"),
        ("kept", "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { port: 18292 })\n\nroute.post(\"/note\", (req) => {\n  state.note = req.json.note\n  return \"kept\"\n})\n"),
        ("logged", "import { flow, route } from \"holon\"\n\nflow(\"logged\", { port: 18293 })\n\nroute.post(\"/say\", (req) => {\n  console.log(\"said\", req.json.word)\n  return \"said\"\n})\n"),
    ];
    for (slug, text) in flows {
        let plan = holon_author::agent::plan(text).unwrap();
        for unit in &plan.units {
            crate::mcp::json_of(&as_ada, "POST", "/unit", Some(Body::Bytes(unit.clone()))).unwrap();
        }
        let preview = send("POST", "/deploy/preview", serde_json::json!({ "slug": slug, "unit": plan.unit }));
        send("POST", "/deploy", serde_json::json!({ "slug": slug, "unit": plan.unit, "document": plan.document, "acknowledge": preview["needs_approval"] }));
    }
    let request = |slug: &str, url: &str, body: serde_json::Value| {
        send("POST", &format!("/deployments/{slug}/request"), serde_json::json!({ "method": "POST", "url": url, "headers": [["content-type", "application/json"]], "body": body.to_string() }));
    };
    request("steps", &format!("/check?also={CANARY}"), serde_json::json!({ "key": CANARY }));
    let (code, said) = as_ada.send("POST", "/trigger/steps/pay", Some(Body::Json(serde_json::json!({ "key": CANARY })))).unwrap();
    assert_eq!(code, 202, "{}", String::from_utf8_lossy(&said));
    request("kept", "/note", serde_json::json!({ "note": KEPT }));
    request("logged", "/say", serde_json::json!({ "word": LOG }));

    let editor = crate::surfaces::COMMANDS.iter().find(|s| s.name == "editor").unwrap();
    for (name, token, may) in [("ada", &ada, [CANARY, KEPT, LOG].as_slice()), ("wes", &wes, [LOG].as_slice()), ("scout", &scout, [KEPT].as_slice())] {
        let called = std::sync::Arc::new(crate::sync::Mutex::new(vec![]));
        let mut app = App::new(None);
        app.attach(Client::new(Box::new(Recorded(InProcess(state, Some(token.clone())), std::sync::Arc::clone(&called)))));
        let mut seen = String::new();
        let mut look = |app: &mut App| {
            seen.push_str(&screen(app, 160, 60));
            seen.push_str(&format!("{:?}", pane(app)));
        };
        app.open_flow("steps");
        look(&mut app);
        app.flow_section(Section::Runs);
        for _ in 0..2 {
            look(&mut app);
            press(&mut app, KeyCode::Right);
            for _ in 0..3 {
                look(&mut app);
                press(&mut app, KeyCode::Right);
            }
            press(&mut app, KeyCode::Left);
            press(&mut app, KeyCode::Left);
            press(&mut app, KeyCode::Left);
            press(&mut app, KeyCode::Down);
        }
        for (slug, sections) in [("kept", [Section::Status, Section::State, Section::History, Section::Runs]), ("logged", [Section::Status, Section::Logs, Section::Runs, Section::History])] {
            app.open_flow(slug);
            for section in sections {
                app.flow_section(section);
                look(&mut app);
                press(&mut app, KeyCode::Right);
                look(&mut app);
                press(&mut app, KeyCode::Right);
                look(&mut app);
                press(&mut app, KeyCode::Left);
                press(&mut app, KeyCode::Left);
            }
        }
        for canary in [CANARY, KEPT, LOG] {
            assert_eq!(seen.contains(canary), may.contains(&canary), "{name} sees {canary}: {}", may.contains(&canary));
        }
        let strays: Vec<String> = called.lock().unwrap().iter().filter(|c| !editor.routes.iter().any(|r| crate::surfaces::calls(r, c))).cloned().collect();
        assert!(strays.is_empty(), "the editor called, as {name}, what surfaces.rs's row of editor doesn't name: {strays:?}");
    }
}

/// R79, as a class: approving a proposal deploys exactly the unit it proposed, or nothing. For
/// every sample, its proposal as the server keeps it (slug, unit, document) plans to the unit
/// proposed, or is refused, never to another; and it plans whenever the server would take it (its
/// document deploys as its unit, deploy.rs `document_is_the_unit`). Its document under another
/// sample's unit, another sample's document under its unit, and one without its document, are each
/// refused.
#[test]
fn a_proposal_is_deployed_as_proposed_or_refused() {
    use super::approvals::proposed_plan;
    let planned: Vec<_> = holon_author::samples::samples().iter().map(|s| (s.name, holon_author::agent::plan(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name)))).collect();
    assert!(planned.len() > 1);
    let mut taken = vec![];
    for (i, (name, plan)) in planned.iter().enumerate() {
        let (_, other) = &planned[(i + 1) % planned.len()];
        assert_ne!(plan.unit, other.unit, "{name}");
        let proposal = |unit: &str, document: &serde_json::Value| serde_json::json!({ "id": 1, "slug": name, "unit": unit, "document": document });
        let server_takes = holon_author::agent::deploys_as(&plan.document).is_ok_and(|(unit, _)| unit == plan.unit);
        match proposed_plan(&proposal(&plan.unit, &plan.document)) {
            Ok((got, _)) => assert_eq!(got.unit, plan.unit, "{name}: the unit proposed"),
            Err(e) => assert!(!server_takes, "{name}: the server takes this proposal, and the terminal can't approve it: {e}"),
        }
        if server_takes {
            taken.push(*name);
        }
        for tampered in [proposal(&other.unit, &plan.document), proposal(&plan.unit, &other.document)] {
            assert!(proposed_plan(&tampered).is_err(), "{name}: tampered, it is refused");
        }
        assert!(proposed_plan(&serde_json::json!({ "slug": name, "unit": plan.unit })).is_err(), "{name}: without its document");
    }
    assert!(taken.len() * 2 > planned.len(), "most samples are proposable: {taken:?}");
}

/// R79, through the Approvals page: an agent proposes a version of "greet", then a person saves
/// another text of it. Approving the proposal deploys what the agent proposed, not the newest
/// save, and the server marks it approved. The agent's next proposal, a flow of its own (an agent
/// doesn't replace a flow a person saved), is declined, saying why, and nothing of it is deployed.
#[test]
fn approving_a_proposal_deploys_what_was_proposed_not_the_newest_save() {
    let mut state = test_state();
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    book.add("claude", crate::accounts::Kind::Agent, "agent", "ada", "ada").unwrap();
    let claude = book.create_token("claude", "the test", "ada").unwrap().0;
    let greet = |flow: &str, says: &str| format!("import {{ action, flow }} from \"holon\"\n\nflow(\"{flow}\")\n\nexport const hi = action(async (name) => \"{says}\")\n");
    // Staged before accounts, which a stage then needs a sign-in for.
    for (flow, says) in [("greet", "hello"), ("greet", "hello, newer"), ("welcome", "goodbye")] {
        crate::tests::stage_text(&state, &greet(flow, says));
    }
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    let state: &'static AppState = Box::leak(Box::new(state));
    let agent = Client::new(Box::new(InProcess(state, Some(claude))));
    let propose = |flow: &str, says: &str| {
        let text = greet(flow, says);
        let plan = holon_author::agent::plan(&text).unwrap();
        let said = agent.post("/deploy", serde_json::json!({ "slug": flow, "unit": plan.unit, "document": plan.document, "why": format!("says {says}") }));
        assert!(said.as_ref().is_ok_and(|s| s["proposal"]["id"].is_u64()), "a proposal: {said:?}");
        plan.unit
    };
    let proposed = propose("greet", "hello");

    // ada saves another text of it, newer than the proposal.
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, Some(ada)))));
    let newer = greet("greet", "hello, newer");
    let newer_plan = holon_author::agent::plan(&newer).unwrap();
    app.client.as_ref().unwrap().save("greet", &newer_plan, &newer, None).unwrap();
    app.refresh();
    let waiting = app.waiting();
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert!(waiting[0].1.starts_with("to review v") && waiting[0].1.contains("claude, an agent") && waiting[0].1.contains("says hello"), "{waiting:?}");

    // The review is the proposal's, with who proposed it and why; enter deploys it.
    app.go(Page::Approvals);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Proposed by claude (an agent)") && shown.contains("Why: says hello"), "{shown}");
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert!(note(&app).starts_with("deployed \"greet\""), "{}", note(&app));
    let deployed = app.client.as_ref().unwrap().deployment("greet").cloned().unwrap();
    assert_eq!(deployed["unit"].as_str(), Some(proposed.as_str()), "the proposed unit, not the newest save ({})", &newer_plan.unit[..12]);
    let approved = state.proposals.all().into_iter().find(|p| p.slug == "greet").unwrap();
    assert_eq!(approved.state, crate::proposals::Decision::Approved);

    // Its next proposal is declined: never without why, and with it the agent's note is kept.
    propose("welcome", "goodbye");
    app.refresh();
    app.go(Page::Approvals);
    screen(&mut app, 140, 40);
    // A flow neither deployed nor saved here: its proposal waits all the same, after ada's save.
    let at = app.waiting().iter().position(|(slug, w)| slug == "welcome" && w.starts_with("to review v")).unwrap_or_else(|| panic!("{:?}", app.waiting()));
    for _ in 0..at {
        press(&mut app, KeyCode::Down);
    }
    screen(&mut app, 140, 40);
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.modal, Some(Modal::Input { .. })), "asks why: {}", note(&app));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::Input { .. })), "still asks why");
    typed(&mut app, "keep saying hello\n");
    assert!(note(&app).contains("Declined welcome") && note(&app).contains("agent gets your note"), "{}", note(&app));
    let declined = state.proposals.all().into_iter().find(|p| p.slug == "welcome").unwrap();
    assert_eq!(declined.state, crate::proposals::Decision::Declined);
    assert!(app.client.as_ref().unwrap().deployment("welcome").is_none(), "nothing of it deployed");
    assert_eq!(app.client.as_ref().unwrap().deployment("greet").unwrap()["unit"].as_str(), Some(proposed.as_str()), "what runs stays");
    assert!(app.waiting().iter().all(|(_, w)| !w.starts_with("to review v")), "{:?}", app.waiting());
}

/// Access changes are decided on the Approvals page, as the web console's Access changes: while
/// one person besides its author approves a change that gives more, ada's binding of dee as an
/// admin waits. ada, its author, can't approve it, and sees withdraw; bob can't decline it
/// without why, and approves it: it is made. ada's next change she withdraws, saying why. cy, a
/// viewer, is told why the changes aren't shown.
#[test]
fn the_approvals_page_decides_access_changes() {
    let mut state = test_state();
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let pw = "correct horse battery";
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", pw).unwrap();
    let bob = book.accept(&book.invite("bob", "admin", None, "ada").unwrap(), pw).unwrap().0;
    book.accept(&book.invite("dee", "viewer", None, "ada").unwrap(), pw).unwrap();
    book.accept(&book.invite("cy", "viewer", None, "ada").unwrap(), pw).unwrap();
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    let state: &'static AppState = Box::leak(Box::new(state));
    let as_ada = InProcess(state, Some(ada.clone()));
    let send = |method: &str, path: &str, body: serde_json::Value| {
        let (code, said) = as_ada.send(method, path, Some(Body::Json(body))).unwrap();
        assert!((200..300).contains(&code), "{method} {path}: {}", String::from_utf8_lossy(&said));
        serde_json::from_slice::<serde_json::Value>(&said).unwrap()
    };
    send("PUT", "/access/approvals", serde_json::json!({ "changes": 1 }));
    let bind = |who: &str| send("POST", "/bindings", serde_json::json!({ "who": { "principal": who }, "role": "admin", "scope": "host", "why": format!("{who} runs the host") }))["proposal"]["id"].as_u64().expect("it waits");
    let first = bind("dee");
    let open = |token: &String| {
        let mut app = App::new(None);
        app.attach(Client::new(Box::new(InProcess(state, Some(token.clone())))));
        app.refresh();
        app.go(Page::Approvals);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        app
    };

    // ada proposed it: hers to withdraw, not to approve.
    let mut a = open(&ada);
    assert_eq!(a.changes_waiting().len(), 1);
    assert_eq!(a.changes_yours(), 0);
    let shown = screen(&mut a, 140, 40);
    assert!(shown.contains("Access changes 1") && shown.contains("admin") && shown.contains("withdraw") && shown.contains("dee runs the host"), "{shown}");
    press(&mut a, KeyCode::Char('a'));
    assert!(a.modal.is_none() && note(&a).contains("you proposed it"), "{}", note(&a));

    // bob: not declined without why; approved, it is made.
    let mut b = open(&bob);
    assert_eq!(b.changes_yours(), 1);
    assert_eq!(b.waiting_total(), 1, "what waits for bob counts it");
    press(&mut b, KeyCode::Char('d'));
    press(&mut b, KeyCode::Enter);
    assert!(matches!(b.modal, Some(Modal::Input { .. })) && note(&b).contains("says why"), "{}", note(&b));
    press(&mut b, KeyCode::Esc);
    press(&mut b, KeyCode::Char('a'));
    let Some(Modal::Confirm { title, lines, .. }) = &b.modal else { panic!("no question: {}", note(&b)) };
    assert_eq!(title, &format!("Approve access change {first}?"));
    assert!(lines[0].contains("It gives") && lines[1].contains("Proposed by ada, saying: dee runs the host") && lines[1].contains("made once you do"), "{lines:?}");
    press(&mut b, KeyCode::Enter);
    assert!(note(&b).contains("made, as access version"), "{}", note(&b));
    assert!(book.bindings_of("dee").iter().any(|x| x.role == "admin"), "dee is an admin now");
    assert!(b.changes_waiting().is_empty());

    // ada's next one, withdrawn with why.
    let second = bind("cy");
    let mut a = open(&ada);
    press(&mut a, KeyCode::Char('d'));
    let Some(Modal::Input { title, .. }) = &a.modal else { panic!("no question: {}", note(&a)) };
    assert_eq!(title, &format!("Withdraw access change {second}?"));
    press(&mut a, KeyCode::Enter);
    assert!(note(&a).contains("Withdrawing says why"), "{}", note(&a));
    typed(&mut a, "the wrong person\n");
    assert!(note(&a).contains(&format!("withdrew access change {second}")), "{}", note(&a));
    let shown = screen(&mut a, 140, 40);
    assert!(shown.contains("withdrawn by ada: the wrong person") && shown.contains("made as version"), "decided lately: {shown}");

    // cy, a viewer, is told why they aren't shown, and has nothing to decide.
    let cy = book.sign_in("cy", pw).unwrap().0;
    let mut c = open(&cy);
    let shown = screen(&mut c, 140, 40);
    assert!(shown.contains("can't be shown here") && shown.contains("needs an admin") && c.changes_yours() == 0, "{shown}");
    for (w, h) in [(80, 24), (60, 16)] {
        screen(&mut c, w, h);
        screen(&mut a, w, h);
    }
}

/// A version whose runs would all wait at an approval no one here can decide (its one step a role
/// nobody holds) is asked about again before it deploys, naming the approval and why; esc goes
/// back to the review, and confirming deploys it as it is.
#[test]
fn deploying_asks_again_when_no_one_here_can_decide_an_approval() {
    let (_, open) = waiting_payouts();
    let (mut ada, _) = open("ada");
    let text = "import { action, approval, flow } from \"holon\"

flow(\"audited\")

export const close = action(async (books) => {
  const decision = await approval(\"audit\", { steps: [{ role: \"auditor\" }], expires: \"1h\" })
  return decision.approved
})
";
    tab_with(&mut ada, text);
    ctrl(&mut ada, 'd');
    assert!(matches!(ada.modal, Some(Modal::Approve { .. })), "{}", note(&ada));
    press(&mut ada, KeyCode::Char('t'));
    press(&mut ada, KeyCode::Enter);
    let Some(Modal::Confirm { title, lines, .. }) = &ada.modal else { panic!("not asked again: {}", note(&ada)) };
    assert!(title.contains("\"audited\" anyway?"), "{title}");
    assert!(lines[0].contains("an approval no one here can decide") && lines.iter().any(|l| l.starts_with("• ") && l.contains("audit")), "{lines:?}");
    screen(&mut ada, 120, 40);
    press(&mut ada, KeyCode::Esc);
    assert!(matches!(ada.modal, Some(Modal::Approve { .. })), "back to the review");
    assert!(ada.client.as_ref().unwrap().deployment("audited").is_none());
    press(&mut ada, KeyCode::Enter);
    press(&mut ada, KeyCode::Char('y'));
    assert!(note(&ada).starts_with("deployed \"audited\""), "{}", note(&ada));
}

/// R121: approving new permissions takes a mark first (t), one rule over every way the editor
/// approves, as the web's tick: a deploy from a tab, a proposal picked on the Approvals page, a
/// restore's dry run. Each says why it waits while unmarked, and goes through once marked. The mark
/// approves only the list it was made for: read afresh (the dialog opened again, another proposal
/// picked and back) it is gone, and a list grown under it is refused, so a mark made for three
/// permissions never approves four. Asking nothing new, each goes at once, no mark asked. Each
/// way's guard taken out, this fails.
#[test]
fn approving_new_permissions_takes_a_mark_first_however_it_is_approved() {
    let deployed = |app: &App| app.note.as_ref().is_some_and(|n| !n.error && n.text.starts_with("deployed"));

    // A deploy from a tab: the first asks for what bump keeps; read afresh, it is opened again.
    let (mut app, _) = app();
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    takes_a_mark("a deploy", &mut app, KeyCode::Char('y'), |app| {
        press(app, KeyCode::Esc);
        ctrl(app, 'd');
    }, deployed);
    app.tabs[0].buf.replace(&BUMP.replace("+ 1", "+ 3"));
    ctrl(&mut app, 'd');
    goes_at_once("a deploy", &mut app, KeyCode::Char('y'), deployed);

    // A proposal picked on the Approvals page: an agent proposes bump and beats, each a first
    // deploy; read afresh, the next is picked and then this one again. Then a version of bump
    // asking nothing new.
    let mut state = test_state();
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    book.add("claude", crate::accounts::Kind::Agent, "agent", "ada", "ada").unwrap();
    let claude = book.create_token("claude", "the test", "ada").unwrap().0;
    let texts = [BUMP.to_string(), BEATS.to_string(), BUMP.replace("+ 1", "+ 2")];
    // Staged before accounts, which a stage then needs a sign-in for.
    for text in &texts {
        crate::tests::stage_text(&state, text);
    }
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    let state: &'static AppState = Box::leak(Box::new(state));
    let agent = Client::new(Box::new(InProcess(state, Some(claude))));
    let propose = |text: &str| {
        let plan = holon_author::agent::plan(text).unwrap();
        let said = agent.post("/deploy", serde_json::json!({ "slug": plan.name, "unit": plan.unit, "document": plan.document, "why": "r121" }));
        assert!(said.as_ref().is_ok_and(|s| s["proposal"]["id"].is_u64()), "a proposal: {said:?}");
    };
    let pick = |app: &mut App, slug: &str| {
        app.refresh();
        app.go(Page::Approvals);
        app.sel[1] = app.waiting().iter().position(|(s, _)| s == slug).unwrap_or_else(|| panic!("{slug} waits: {:?}", app.waiting()));
        screen(app, 140, 40);
    };
    propose(&texts[0]);
    propose(&texts[1]);
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, Some(ada)))));
    pick(&mut app, "bump");
    takes_a_mark("a picked proposal", &mut app, KeyCode::Enter, |app| {
        for k in [']', '['] {
            press(app, KeyCode::Char(k));
            screen(app, 140, 40);
        }
    }, deployed);
    propose(&texts[2]);
    pick(&mut app, "bump");
    goes_at_once("a picked proposal", &mut app, KeyCode::Enter, deployed);

    // A restore's dry run: bump's first version keeps one more key than its second, so going back
    // to its run asks for it again; read afresh, the dry run is shown again. Then going back to the
    // same run, its code deployed now, asks nothing new.
    let (mut app, _) = super::tests::app();
    tab_with(&mut app, &BUMP.replace("  return n", "  state.last = n\n  return n"));
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Char('y'));
    assert!(deployed(&app), "{}", note(&app));
    let (_, failed) = app.client.as_ref().unwrap().action("bump", "bump", "{}").unwrap();
    assert!(!failed);
    app.tabs[0].buf.replace(BUMP);
    ctrl(&mut app, 'd');
    goes_at_once("a deploy keeping less", &mut app, KeyCode::Char('y'), deployed);
    let restored = |app: &App| app.note.as_ref().is_some_and(|n| !n.error && n.text.contains("live again"));
    go_back_to_the_first_run(&mut app);
    takes_a_mark("a restore", &mut app, KeyCode::Char('y'), |app| {
        press(app, KeyCode::Esc);
        go_back_to_the_first_run(app);
    }, restored);
    go_back_to_the_first_run(&mut app);
    goes_at_once("a restore", &mut app, KeyCode::Char('y'), restored);
}

/// The review of what `app` approves now: a dialog's, else the Approvals page's pick's.
fn review_of(app: &App) -> Option<&super::review::Review> {
    match (&app.modal, &app.approval) {
        (Some(Modal::Approve { review, .. } | Modal::DryRun { review, .. }), _) => Some(review),
        (None, Some(super::app::Approval { what: super::app::Pending::Deploy(s), .. })) => Some(&s.review),
        _ => None,
    }
}

/// The new permissions approving it would grant, as it would send them.
fn approving(app: &mut App) -> Option<&mut Vec<String>> {
    match (&mut app.modal, &mut app.approval) {
        (Some(Modal::Approve { staged, .. }), _) => Some(&mut staged.needs),
        (Some(Modal::DryRun { needs, .. }), _) => Some(needs),
        (None, Some(super::app::Approval { what: super::app::Pending::Deploy(s), .. })) => Some(&mut s.staged.needs),
        _ => None,
    }
}

/// The review's last line: its mark, when it asks for one.
fn mark_line(app: &App) -> String {
    review_of(app).and_then(|r| r.said().last().map(|(_, l)| l.clone())).unwrap_or_default()
}

/// The rule through one way to approve: `app` shows something that asks for new permissions; `go`
/// approves it, `again` reads it afresh, `went` says it went through.
fn takes_a_mark(way: &str, app: &mut App, go: KeyCode, again: impl Fn(&mut App), went: impl Fn(&App) -> bool) {
    let asks = approving(app).unwrap_or_else(|| panic!("{way}: nothing to approve")).clone();
    assert!(!asks.is_empty(), "{way}: it asks for nothing new");
    assert_eq!(review_of(app).unwrap().asks, asks, "{way}: the review counts what approving grants");
    let unmarked = |app: &App| mark_line(app).starts_with("[ ] I read the");
    let approve = |app: &mut App| {
        app.note = None;
        press(app, go);
    };
    let refused = |app: &App, why: &str| !went(app) && app.note.as_ref().is_some_and(|n| n.error && n.text.contains(why));
    assert!(unmarked(app), "{way}: {}", mark_line(app));
    approve(app);
    assert!(refused(app, "Not yet: mark that you read"), "{way}: approved unmarked: {}", note(app));
    press(app, KeyCode::Char('t'));
    assert!(mark_line(app).starts_with("[x] I read the"), "{way}: {}", mark_line(app));
    again(app);
    assert!(unmarked(app), "{way}: read afresh, the mark is gone: {}", mark_line(app));
    approve(app);
    assert!(refused(app, "Not yet: mark that you read"), "{way}: approved after it was read afresh: {}", note(app));
    press(app, KeyCode::Char('t'));
    approving(app).unwrap().push("State(\"more\", write)".into());
    approve(app);
    assert!(refused(app, "changed since you marked"), "{way}: a mark for {} approved {}: {}", asks.len(), asks.len() + 1, note(app));
    approving(app).unwrap().pop();
    approve(app);
    assert!(went(app), "{way}: marked, it goes: {}", note(app));
}

/// Asking nothing new, `go` approves at once, no mark asked.
fn goes_at_once(way: &str, app: &mut App, go: KeyCode, went: impl Fn(&App) -> bool) {
    assert_eq!(approving(app).map(|n| n.len()), Some(0), "{way}: {}", note(app));
    assert!(!mark_line(app).contains("I read the"), "{way}: {}", mark_line(app));
    app.note = None;
    press(app, go);
    assert!(went(app), "{way}: nothing new, it goes at once: {}", note(app));
}

/// bump's history, its first run picked, both put back: the dry run.
fn go_back_to_the_first_run(app: &mut App) {
    app.refresh();
    app.open_flow("bump");
    app.flow_section(Section::History);
    let d = app.detail.as_ref().unwrap();
    let first = d.runs.iter().filter_map(|r| r["seq"].as_u64()).min().unwrap();
    app.flow.sel[Section::History as usize] = d.moments().iter().position(|m| matches!(m, Moment::Run(i) if d.runs[*i]["seq"] == first)).unwrap();
    app.flow_load();
    press(app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::Restore { .. })), "{}", note(app));
    press(app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::DryRun { .. })), "{}", note(app));
}

/// v in the deploy review replays the version offline on the flow's last runs, and says which
/// answer differently: bump's next version answers ten times as much, so each of its three runs
/// differs. A first deploy has nothing to replay on, and says so.
#[test]
fn the_deploy_review_replays_a_version_on_recent_runs() {
    let (mut app, _) = bumped(3);
    tab_with(&mut app, &BUMP.replace("return n\n", "return n * 10\n"));
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('v'));
    let Some(Modal::Approve { review, .. }) = &app.modal else { panic!("no dialog: {}", note(&app)) };
    let top: Vec<&str> = review.top.iter().map(|(_, l)| l.as_str()).collect();
    assert!(top[0].contains("replayed on \"bump\"'s last 3 runs") && top[0].contains("3 differ"), "{top:?}");
    assert_eq!(top.iter().filter(|l| l.starts_with("  run ") && l.contains("differs")).count(), 3, "{top:?}");
    // Again: in place of the first, not beside it.
    press(&mut app, KeyCode::Char('v'));
    let Some(Modal::Approve { review, .. }) = &app.modal else { panic!() };
    assert_eq!(review.top.iter().filter(|(_, l)| l.contains("replayed on")).count(), 1);
    screen(&mut app, 120, 40);
    // :replay does it from the tab, deploying nothing.
    press(&mut app, KeyCode::Esc);
    app.run_command("replay");
    let Some(Modal::Lines { title, lines, .. }) = &app.modal else { panic!("no lines: {}", note(&app)) };
    assert!(title.contains("bump") && lines[0].1.contains("3 differ"), "{title} {lines:?}");

    let (mut app, _) = super::tests::app();
    tab_with(&mut app, BEATS);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('v'));
    let Some(Modal::Approve { review, .. }) = &app.modal else { panic!() };
    assert!(review.top[0].1.contains("isn't deployed yet"), "{:?}", review.top);
}

/// The approvals steps run live: against a real server at `HOLON_LIVE` (a fresh one, started with
/// `HOLON_SETUP_CODE=HOLON-LIVE`), over HTTP as the editor speaks to it. It sets the server up (ada,
/// bob, an agent), runs a flow that waits at a two-person approval, and the editor decides it; the
/// agent proposes a flow and ada's editor deploys it as proposed; an access change waits and bob's
/// editor approves it. Every page it passes is written to `HOLON_SCREENS`, for a person to read.
#[test]
#[ignore = "runs against a live server at HOLON_LIVE"]
fn live_approvals() {
    let server = std::env::var("HOLON_LIVE").expect("HOLON_LIVE: the server's address");
    let out = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write the pages");
    let pw = "correct horse battery";
    let open = |token: Option<&str>| Client::new(Box::new(crate::mcp::Http::new(&server, token)));
    let ok = |said: Result<serde_json::Value, String>, what: &str| said.unwrap_or_else(|e| panic!("{what}: {e}"));
    let ada = ok(open(None).post("/auth/setup", serde_json::json!({ "code": "HOLON-LIVE", "name": "ada", "password": pw })), "setup")["token"].as_str().unwrap().to_string();
    let as_ada = open(Some(&ada));
    let invite = ok(as_ada.post("/people", serde_json::json!({ "name": "bob", "kind": "person", "role": "admin" })), "bob")["invite"].as_str().unwrap().to_string();
    let bob = ok(open(None).post("/auth/invite", serde_json::json!({ "token": invite, "password": pw })), "bob joins")["token"].as_str().unwrap().to_string();
    let invite = ok(as_ada.post("/people", serde_json::json!({ "name": "cy", "kind": "person", "role": "admin" })), "cy")["invite"].as_str().unwrap().to_string();
    let cy = ok(open(None).post("/auth/invite", serde_json::json!({ "token": invite, "password": pw })), "cy joins")["token"].as_str().unwrap().to_string();
    ok(as_ada.post("/people", serde_json::json!({ "name": "claude", "kind": "agent", "role": "agent" })), "claude");
    let agent = ok(as_ada.post("/people/claude/tokens", serde_json::json!({ "label": "live" })), "claude's token")["token"].as_str().unwrap().to_string();
    let stage = |token: &str, text: &str| {
        let plan = holon_author::agent::plan(text).unwrap();
        for u in &plan.units {
            let (code, said) = crate::mcp::Http::new(&server, Some(token)).send("POST", "/unit", Some(Body::Bytes(u.clone()))).unwrap();
            assert!((200..300).contains(&code), "staged: {}", String::from_utf8_lossy(&said));
        }
        plan
    };
    // Each page as it passes, so a failure leaves what led to it.
    let mut pages = String::new();
    let mut keep = |name: &str, app: &mut App| {
        pages += &format!("\n===== {name} =====\n{}\nnote: {}\n", screen(app, 140, 40), note(app));
        std::fs::write(&out, &pages).unwrap();
    };
    let editor = |token: &str| {
        let mut app = App::new(None);
        app.attach(open(Some(token)));
        app.go(Page::Approvals);
        app
    };

    // Runs waiting at a two-person approval: ada starts it, bob and cy approve it, and it goes on.
    let payouts = "import { action, approval, flow } from \"holon\"\n\nflow(\"payouts\")\n\nexport const pay = action(async (order) => {\n  const decision = await approval(\"payout\", { expires: \"1h\", people: 2 })\n  return decision.approved\n})\n";
    let plan = stage(&ada, payouts);
    let ack = serde_json::json!({ "slug": "payouts", "unit": plan.unit, "document": plan.document, "acknowledge": ["Action(\"pay\")", "Approval(\"payout/person*2\")"] });
    ok(as_ada.post("/deploy", ack), "payouts deployed");
    ok(as_ada.post("/trigger/payouts/pay", serde_json::json!({ "amount": 10 })).or_else(|e| if e.contains("wait") { Ok(serde_json::Value::Null) } else { Err(e) }), "a run");
    // ada started it: it waits, and none of it is hers to decide.
    let mut a = editor(&ada);
    press(&mut a, KeyCode::Tab);
    keep("ada: Runs waiting, hers", &mut a);
    assert_eq!((a.runs_waiting().len(), a.runs_yours()), (1, 0), "{}", note(&a));
    press(&mut a, KeyCode::Char('a'));
    assert!(a.modal.is_none(), "not hers: {}", note(&a));
    let mut b = editor(&bob);
    press(&mut b, KeyCode::Tab);
    keep("bob: Runs waiting", &mut b);
    assert_eq!(b.runs_yours(), 1, "{}", note(&b));
    press(&mut b, KeyCode::Char('a'));
    typed(&mut b, "fine by me\n");
    keep("bob approved", &mut b);
    assert!(note(&b).contains("1 more person"), "{}", note(&b));
    let mut c = editor(&cy);
    press(&mut c, KeyCode::Tab);
    press(&mut c, KeyCode::Char('a'));
    press(&mut c, KeyCode::Enter);
    keep("cy approved", &mut c);
    assert!(note(&c).contains("goes on"), "{}", note(&c));

    // The agent proposes a flow; ada's editor shows who and why, and deploys what was proposed.
    let as_agent = open(Some(&agent));
    let greet = "import { action, flow } from \"holon\"\n\nflow(\"greet\")\n\nexport const hi = action(async (name) => \"hello\")\n";
    let plan = stage(&agent, greet);
    let said = ok(as_agent.post("/deploy", serde_json::json!({ "slug": "greet", "unit": plan.unit, "document": plan.document, "why": "to say hello" })), "proposed");
    assert!(said["proposal"]["id"].is_u64(), "{said}");
    let mut a = editor(&ada);
    keep("ada: Deploys, the proposal", &mut a);
    let shown = screen(&mut a, 140, 40);
    assert!(shown.contains("Proposed by claude (an agent)") && shown.contains("Why: to say hello"), "{shown}");
    press(&mut a, KeyCode::Char('v'));
    keep("ada: replayed", &mut a);
    press(&mut a, KeyCode::Char('t'));
    press(&mut a, KeyCode::Enter);
    keep("ada deployed it", &mut a);
    assert!(note(&a).starts_with("deployed \"greet\""), "{}", note(&a));
    assert_eq!(a.client.as_ref().unwrap().deployment("greet").unwrap()["unit"].as_str(), Some(plan.unit.as_str()));

    // An access change waits for a second person; bob's editor approves it.
    let (code, said) = crate::mcp::Http::new(&server, Some(&ada)).send("PUT", "/access/approvals", Some(Body::Json(serde_json::json!({ "changes": 1 })))).unwrap();
    assert!((200..300).contains(&code), "changes wait: {}", String::from_utf8_lossy(&said));
    ok(as_ada.post("/people", serde_json::json!({ "name": "dee", "kind": "person", "role": "viewer" })), "dee");
    ok(as_ada.post("/bindings", serde_json::json!({ "who": { "principal": "dee" }, "role": "admin", "scope": "host", "why": "dee runs the host" })), "the change");
    let mut b = editor(&bob);
    press(&mut b, KeyCode::Tab);
    press(&mut b, KeyCode::Tab);
    keep("bob: Access changes", &mut b);
    assert_eq!(b.changes_yours(), 1, "{}", note(&b));
    press(&mut b, KeyCode::Char('a'));
    press(&mut b, KeyCode::Enter);
    keep("bob approved the change", &mut b);
    assert!(note(&b).contains("made, as access version"), "{}", note(&b));
}

/// The row of `app`'s Settings that `pick` finds, picked.
fn pick_setting(app: &mut App, pick: impl Fn(&super::settings::Setting) -> bool) {
    let d = app.detail.as_ref().expect("a flow's page");
    let i = super::settings::settings_rows(d).iter().position(pick).unwrap_or_else(|| panic!("no such row: {:?}", super::settings::settings_rows(d)));
    app.flow.sel[Section::Settings as usize] = i;
    app.flow_load();
}

/// A flow's Settings, as the web console's: a limit lowered at once and a higher one refused
/// (a new version raises it); keep running turned on; agents' proposals turned off; the flow stopped, then run again from a session's state; a secret rolled
/// back after its dry run; and the flow removed, its name typed.
#[test]
fn a_flows_settings_lower_its_limits_turn_its_switches_and_remove_it() {
    use super::flow::{access_rows, Access};
    use super::settings::Setting;
    let (mut app, state) = bumped(2);
    app.open_flow("bump");
    app.flow_section(Section::Settings);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Settings") && shown.contains("newest runs kept") && shown.contains("Agents"), "{shown}");

    // keep_runs: lowered, then not raised.
    pick_setting(&mut app, |s| matches!(s, Setting::Limit { name, .. } if name == "keep_runs"));
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "5\n");
    assert!(note(&app).contains("keep_runs lowered to 5"), "{}", note(&app));
    let d = app.detail.as_ref().unwrap();
    assert!(super::settings::settings_rows(d).iter().any(|s| matches!(s, Setting::Limit { name, now: 5, lowered: Some(5), .. } if name == "keep_runs")), "{:?}", super::settings::settings_rows(d));
    pick_setting(&mut app, |s| matches!(s, Setting::Limit { name, .. } if name == "keep_runs"));
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "50\n");
    assert!(note(&app).contains("raising it takes a new version"), "{}", note(&app));

    // Keep running, on.
    pick_setting(&mut app, |s| matches!(s, Setting::KeepRunning(false)));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert!(app.detail.as_ref().unwrap().grant["grant"]["keep_running"] == true, "{}", note(&app));

    // Agents may no longer propose versions.
    pick_setting(&mut app, |s| matches!(s, Setting::AgentsPropose(true)));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert!(note(&app).contains("may no longer propose"), "{}", note(&app));
    assert!(app.client.as_ref().unwrap().deployment("bump").unwrap()["agents_propose"] == false);

    // A secret set twice, rolled back to its first version after a dry run.
    let c = app.client.as_ref().unwrap();
    c.set_secret("bump", "token", "first-value").unwrap();
    c.set_secret("bump", "token", "second-value").unwrap();
    app.refresh();
    app.flow_section(Section::Access);
    let at = access_rows(app.detail.as_ref().unwrap()).iter().position(|r| matches!(r, Access::Secret { name, set: true } if name == "token")).expect("its secret");
    app.flow.sel[Section::Access as usize] = at;
    press(&mut app, KeyCode::Char('R'));
    typed(&mut app, "1\n");
    let Some(Modal::Confirm { title, lines, .. }) = &app.modal else { panic!("no dry run: {}", note(&app)) };
    assert!(title.contains("Roll token back to v1") && lines[0].contains("would make v1"), "{title} {lines:?}");
    press(&mut app, KeyCode::Enter);
    assert!(note(&app).contains("token is v1 again, as v3"), "{}", note(&app));

    // Stopped, then run again from its first session's state.
    app.lifecycle("bump", "stop", None);
    app.flow_section(Section::Settings);
    pick_setting(&mut app, |s| matches!(s, Setting::Session { state: true, ended: Some(_), .. }));
    screen(&mut app, 140, 40);
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Confirm { title, .. }) = &app.modal else { panic!("not asked: {}", note(&app)) };
    assert!(title.starts_with("Run bump from session"), "{title}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.client.as_ref().unwrap().deployment("bump").unwrap()["status"], "running", "{}", note(&app));

    // Removed: not while it runs, as the web console; then not without its name typed.
    pick_setting(&mut app, |s| matches!(s, Setting::Remove { forget: false }));
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none() && note(&app).contains("once it is stopped"), "{}", note(&app));
    app.lifecycle("bump", "stop", None);
    pick_setting(&mut app, |s| matches!(s, Setting::Remove { forget: false }));
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "bmup\n");
    assert!(note(&app).contains("type bump to remove it"), "{}", note(&app));
    assert!(app.client.as_ref().unwrap().deployment("bump").is_some());
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "bump\n");
    assert!(note(&app).starts_with("removed \"bump\""), "{}", note(&app));
    assert!(!state.deployments.lock().unwrap().contains_key("bump"));
}

/// A flow's Settings run live: against a fresh server at `HOLON_LIVE` (started with
/// `HOLON_SETUP_CODE=HOLON-LIVE`), over HTTP as the editor speaks to it. ada deploys `bump` from a
/// tab, runs it twice, lowers a limit, turns keep running on and agents' proposals off, rolls a
/// secret back after its dry run, runs it again from its first session, and removes it once
/// stopped, its name typed. Each page it passes is written to `HOLON_SCREENS`.
#[test]
#[ignore = "runs against a live server at HOLON_LIVE"]
fn live_settings() {
    use super::flow::{access_rows, Access};
    use super::settings::Setting;
    let server = std::env::var("HOLON_LIVE").expect("HOLON_LIVE: the server's address");
    let out = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write the pages");
    let setup = Client::new(Box::new(crate::mcp::Http::new(&server, None))).post("/auth/setup", serde_json::json!({ "code": "HOLON-LIVE", "name": "ada", "password": "correct horse battery" }));
    let ada = setup.unwrap_or_else(|e| panic!("setup: {e}"))["token"].as_str().unwrap().to_string();
    let mut pages = String::new();
    let mut keep = |name: &str, app: &mut App| {
        pages += &format!("\n===== {name} =====\n{}\nnote: {}\n", screen(app, 140, 40), note(app));
        std::fs::write(&out, &pages).unwrap();
    };
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(crate::mcp::Http::new(&server, Some(&ada)))));
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    keep("deployed from a tab", &mut app);
    for _ in 0..2 {
        app.client.as_ref().unwrap().post("/trigger/bump/bump", serde_json::json!({})).unwrap();
    }
    app.refresh();
    app.open_flow("bump");
    app.flow_section(Section::Settings);
    keep("Settings", &mut app);

    pick_setting(&mut app, |s| matches!(s, Setting::Limit { name, .. } if name == "keep_runs"));
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "5\n");
    keep("keep_runs lowered", &mut app);
    assert!(note(&app).contains("keep_runs lowered to 5"), "{}", note(&app));
    pick_setting(&mut app, |s| matches!(s, Setting::Limit { name, .. } if name == "keep_runs"));
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "50\n");
    assert!(note(&app).contains("raising it takes a new version"), "{}", note(&app));

    pick_setting(&mut app, |s| matches!(s, Setting::KeepRunning(false)));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    keep("keep running on", &mut app);
    assert!(app.detail.as_ref().unwrap().grant["grant"]["keep_running"] == true, "{}", note(&app));
    pick_setting(&mut app, |s| matches!(s, Setting::AgentsPropose(true)));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    keep("agents may no longer propose", &mut app);
    assert!(note(&app).contains("may no longer propose"), "{}", note(&app));

    let c = app.client.as_ref().unwrap();
    c.set_secret("bump", "token", "first-value").unwrap();
    c.set_secret("bump", "token", "second-value").unwrap();
    app.refresh();
    app.flow_section(Section::Access);
    let at = access_rows(app.detail.as_ref().unwrap()).iter().position(|r| matches!(r, Access::Secret { name, set: true } if name == "token")).expect("its secret");
    app.flow.sel[Section::Access as usize] = at;
    press(&mut app, KeyCode::Char('R'));
    typed(&mut app, "1\n");
    keep("the roll-back's dry run", &mut app);
    assert!(matches!(&app.modal, Some(Modal::Confirm { title, .. }) if title.contains("Roll token back to v1")), "{}", note(&app));
    press(&mut app, KeyCode::Enter);
    keep("rolled back", &mut app);
    assert!(note(&app).contains("token is v1 again, as v3"), "{}", note(&app));

    app.lifecycle("bump", "stop", None);
    app.flow_section(Section::Settings);
    pick_setting(&mut app, |s| matches!(s, Setting::Session { state: true, ended: Some(_), .. }));
    keep("a session picked", &mut app);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    keep("run from its session", &mut app);
    assert_eq!(app.client.as_ref().unwrap().deployment("bump").unwrap()["status"], "running", "{}", note(&app));

    // Its graph, over the text the server has, at each level.
    app.flow_section(Section::Graph);
    for level in [Level::Statements, Level::Grouped, Level::Nodes] {
        press(&mut app, KeyCode::Char('a'));
        keep(&format!("Graph, {}", level.name()), &mut app);
        assert_eq!(app.flow.graph.as_ref().map(|g| g.level), Some(level), "{}", note(&app));
    }
    assert!(app.flow.graph.as_ref().is_some_and(|g| g.error.is_none() && g.rows.len() > 4));
    app.flow_section(Section::Settings);

    pick_setting(&mut app, |s| matches!(s, Setting::Remove { forget: false }));
    press(&mut app, KeyCode::Enter);
    assert!(note(&app).contains("once it is stopped"), "{}", note(&app));
    app.lifecycle("bump", "stop", None);
    pick_setting(&mut app, |s| matches!(s, Setting::Remove { forget: false }));
    press(&mut app, KeyCode::Enter);
    typed(&mut app, "bump\n");
    keep("removed", &mut app);
    assert!(note(&app).starts_with("removed \"bump\""), "{}", note(&app));
    assert!(app.client.as_ref().unwrap().deployment("bump").is_none());
}

/// A server with accounts, set up by ada (an admin), and `people` invited, each `(name, role)`:
/// the server, its accounts, and each one's sign-in by name.
fn people_server(people: &[(&str, &str)]) -> (&'static AppState, std::sync::Arc<crate::accounts::Accounts>, std::collections::HashMap<String, String>) {
    people_server_on(test_state(), people)
}

/// As `people_server`, on `state`.
fn people_server_on(mut state: AppState, people: &[(&str, &str)]) -> (&'static AppState, std::sync::Arc<crate::accounts::Accounts>, std::collections::HashMap<String, String>) {
    let book = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let pw = "correct horse battery";
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", pw).unwrap();
    state.access = crate::access::Access::new(std::sync::Arc::clone(&book), vec![], vec![]);
    let mut tokens = std::collections::HashMap::from([("ada".to_string(), ada)]);
    for (name, role) in people {
        tokens.insert(name.to_string(), book.accept(&book.invite(name, role, None, "ada").unwrap(), pw).unwrap().0);
    }
    (Box::leak(Box::new(state)), book, tokens)
}

/// The editor on `state`'s admin page, signed in with `token`, its calls recorded.
fn admin_page_as(state: &'static AppState, token: &str) -> (App, Calls) {
    let calls: Calls = std::sync::Arc::new(crate::sync::Mutex::new(vec![]));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(Recorded(InProcess(state, Some(token.to_string())), std::sync::Arc::clone(&calls)))));
    app.refresh();
    press(&mut app, KeyCode::Char('7'));
    assert_eq!(app.page, Page::Admin);
    (app, calls)
}

/// The admin page's right pane, its rows' words each on a line, however narrow the pane.
fn detail(app: &App) -> String {
    let rows = app.admin.pane.as_ref().map(|(_, rows)| rows.as_slice()).unwrap_or_default();
    rows.iter()
        .map(|r| match r {
            Row::Text(_, t) => t.clone(),
            Row::Field(k, v, _) => format!("{k}: {v}"),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("
")
}

/// A group made, a member added and a role bound to it on one flow; a role defined with two
/// actions; each change on the access history, whose versions open whole.
#[test]
fn the_access_parts_make_a_group_define_a_role_and_bind_it() {
    let (state, _, tokens) = people_server(&[("ana", "editor"), ("bob", "editor")]);
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.admin_part(Part::Groups);
    assert!(screen(&mut app, 140, 40).contains("No groups"));
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payments ana --about the payments team --why for the pilot");
    let g = app.admin.groups.first().unwrap_or_else(|| panic!("not made: {}", note(&app)));
    assert_eq!((&g["name"], &g["members"], &g["about"]), (&serde_json::json!("payments"), &serde_json::json!(["ana"]), &serde_json::json!("the payments team")));
    press(&mut app, KeyCode::Char('+'));
    answer(&mut app, "bob");
    assert_eq!(app.admin.groups[0]["members"], serde_json::json!(["ana", "bob"]), "{}", note(&app));
    // A group by a name already taken isn't made again over it.
    let read_at = app.admin.groups_version.clone();
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payments cy");
    assert!(app.note.as_ref().is_some_and(|n| n.error) && note(&app).contains("+ adds members"), "{}", note(&app));
    assert_eq!(app.admin.groups_version, read_at);

    app.admin_part(Part::Roles);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payout-approver run.view,run.decide --kinds person --about approves payouts");
    let role = app.admin.roles.iter().find(|r| r["name"] == "payout-approver").unwrap_or_else(|| panic!("not defined: {}", note(&app)));
    assert_eq!(role["allows"].as_array().map(Vec::len), Some(2), "{role}");
    let at = app.admin.roles.iter().position(|r| r["name"] == "payout-approver").unwrap();
    app.admin_pick(List::Admin, at, false);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Allows 2 actions") && shown.contains("run.decide") && shown.contains("approves payouts"), "{shown}");
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "nope run.nothing");
    assert!(note(&app).contains("isn't an action"), "{}", note(&app));

    app.admin_part(Part::Bindings);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payout-approver --group payments --flow payout --why the payout team decides");
    let b = app.admin.bindings.iter().find(|b| b["who"]["group"] == "payments").unwrap_or_else(|| panic!("not bound: {}", note(&app)));
    assert_eq!((&b["role"], &b["scope"]["flows"]["slug"]), (&serde_json::json!("payout-approver"), &serde_json::json!("payout")));
    assert!(note(&app).starts_with("bound payout-approver to the group payments on \"payout\""), "{}", note(&app));
    // Its members hold it there, through the group, as their own page says.
    app.admin_part(Part::People);
    let ana = app.admin.users.iter().position(|u| u["name"] == "ana").unwrap();
    app.admin_pick(List::Admin, ana, false);
    let shown = detail(&app);
    assert!(shown.contains("payout-approver on \"payout\", through payments"), "{shown}");
    app.admin_part(Part::Groups);
    assert!(detail(&app).contains("1 binding"), "the group says what is bound to it: {}", detail(&app));

    app.admin_part(Part::History);
    assert!(app.admin.versions.len() >= 4, "{:?}", app.admin.versions);
    let shown = detail(&app);
    assert!(shown.contains("Who approves what") && shown.contains("an access change applies at once"), "{shown}");
    press(&mut app, KeyCode::Down);
    let shown = detail(&app);
    assert!(shown.contains("What: bound payout-approver to") && shown.contains("payments: ana, bob") && shown.contains("payout-approver (person): run.view, run.decide"), "the newest version, whole: {shown}");
    screen(&mut app, 80, 24);

    // Unbound, saying why.
    app.admin_part(Part::Bindings);
    let at = app.admin.bindings.iter().position(|b| b["who"]["group"] == "payments").unwrap();
    app.admin_pick(List::Admin, at, false);
    press(&mut app, KeyCode::Delete);
    answer(&mut app, "the pilot is over");
    assert!(!app.admin.bindings.iter().any(|b| b["who"]["group"] == "payments" && b["expired"] != true), "{}", note(&app));
}

/// On a host where an access change that gives more waits for one person besides its author, a
/// binding ada makes waits as a proposal: the line says so and where it is decided, and nothing is
/// bound yet.
#[test]
fn a_binding_that_gives_more_waits_on_a_host_that_requires_approval() {
    let (state, _, tokens) = people_server(&[("bob", "admin"), ("ana", "editor")]);
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.admin_part(Part::History);
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Input { value, .. }) = &app.modal else { panic!("no dialog: {}", note(&app)) };
    assert_eq!(value, "--changes off --exclude-deployer off --agents on --starter on", "the settings as they are");
    answer(&mut app, "--changes 1 --why two people see each change");
    assert!(note(&app).starts_with("approval settings changed: --changes 1"), "{}", note(&app));
    assert!(detail(&app).contains("waits until 1 person other than its author"), "{}", detail(&app));

    app.admin_part(Part::Bindings);
    let before = app.admin.bindings.len();
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "admin --to ana --until 7d --why covering for bob");
    assert!(note(&app).contains("Approvals › Access changes decides it"), "{}", note(&app));
    assert_eq!(app.admin.bindings.len(), before, "nothing bound while it waits");
    let waiting = app.client.as_ref().unwrap().get("/access/proposals").unwrap();
    assert!(waiting["proposals"].as_array().is_some_and(|p| p.len() == 1), "{waiting}");
}

/// Someone whose role holds role.view alone sees the roles, and none of the keys that change
/// them; a key pressed anyway says what it takes and sends nothing. The other access parts say
/// why they aren't shown.
#[test]
fn someone_holding_role_view_only_sees_the_roles_and_no_change_keys() {
    let (state, book, _) = people_server(&[]);
    book.define_role(crate::bindings::RoleText { name: "role-reader".into(), about: None, allows: vec!["role.view".into()], kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let rita = book.accept(&book.invite("rita", "role-reader", None, "ada").unwrap(), "correct horse battery").unwrap().0;
    let (mut app, calls) = admin_page_as(state, &rita);
    app.admin_part(Part::Roles);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("viewer") && shown.contains("role-reader") && shown.contains("Allows"), "{shown}");
    for key in ["define a role", "change it", "remove"] {
        assert!(!shown.contains(key), "no {key:?} key: {shown}");
    }
    let before = calls.lock().unwrap().len();
    press(&mut app, KeyCode::Char('n'));
    assert!(app.modal.is_none() && note(&app).contains("role.define"), "{}", note(&app));
    assert_eq!(calls.lock().unwrap().len(), before, "sends nothing");
    for part in [Part::People, Part::Groups, Part::Bindings, Part::History] {
        app.admin_part(part);
        let shown = screen(&mut app, 140, 40);
        assert!(shown.contains("It can't be shown here") && shown.contains("people.view"), "{part:?}: {shown}");
        assert!(!shown.contains("new group") && !shown.contains("bind a role"), "{part:?}: {shown}");
    }
    assert!(!calls.lock().unwrap().iter().any(|c| c.starts_with("GET /groups") || c.starts_with("GET /bindings")), "nor read for her");
}

/// An agent's owner, no admin, makes an API key for it on Your agents, and revokes it.
#[test]
fn an_agents_owner_makes_and_revokes_its_keys() {
    let (state, book, tokens) = people_server(&[("ana", "editor")]);
    book.add("claude", crate::accounts::Kind::Agent, "agent", "ana", "ada").unwrap();
    let (mut app, _) = admin_page_as(state, &tokens["ana"]);
    app.admin_part(Part::Yours);
    assert!(screen(&mut app, 140, 40).contains("claude"));
    assert!(detail(&app).contains("holon mcp setup claude-code --server the test server"), "{}", detail(&app));
    assert!(!screen(&mut app, 140, 40).contains("People 0"), "a part not shown to ana counts nothing");
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "nightly report");
    let Some(Modal::Lines { title, .. }) = &app.modal else { panic!("no key: {}", note(&app)) };
    assert_eq!(title, "claude: an API key");
    assert!(app.to_clipboard.is_some());
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.admin.owned[0]["tokens"].as_array().map(Vec::len), Some(1));
    // Its newest key is offered: enter revokes it.
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.admin.owned[0]["tokens"].as_array().map(Vec::len), Some(0), "{}", note(&app));
}


/// A server whose policy allows plain http to private addresses (for stand-ins on this machine),
/// set up by ada, with `people` invited: as `people_server` makes it, from `state`.
fn host_server(state: AppState, people: &[(&str, &str)]) -> (&'static AppState, std::sync::Arc<crate::accounts::Accounts>, std::collections::HashMap<String, String>) {
    let policy = "[grants]\nallow_plain_http = true\nallow_private_addresses = true\n";
    // As a policy read from its file keeps its text.
    let parsed = crate::permissions::Policy::parse(policy).unwrap();
    *state.policy.write().unwrap() = std::sync::Arc::new(crate::permissions::Policy { text: Some(policy.into()), ..parsed });
    people_server_on(state, people)
}

/// A queue bound for one deployment only, with holon queue add's flags, read back as holon
/// queue list says it, then unbound.
#[test]
fn a_queue_is_bound_for_one_deployment_and_unbound() {
    let sqs = crate::sqs::stand_in::StandIn::start();
    sqs.make("jobs");
    let mut state = test_state();
    let url = sqs.url("jobs");
    let queues = crate::queues::Queues::memory().with_sources(crate::secret_sources::Sources::of(&[("HOLON_SECRET_JOBS_URL", url.as_str())]));
    state.folders = std::sync::Arc::new(crate::folders::Folders::memory().with_queues(queues));
    crate::tests::stage_text(&state, BUMP);
    let (state, _, tokens) = host_server(state, &[]);
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.go(Page::Editor);
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert!(app.client.as_ref().unwrap().deployment("bump").is_some(), "{}", note(&app));
    press(&mut app, KeyCode::Esc);
    app.go(Page::Admin);
    app.admin_part(Part::Queues);
    assert!(screen(&mut app, 140, 40).contains("No queues bound"));
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "jobs --url env:HOLON_SECRET_JOBS_URL --visibility 2m --keep-failed --for bump");
    let q = app.admin.queues.first().unwrap_or_else(|| panic!("not bound: {}", note(&app)));
    assert_eq!((&q["name"], &q["deployment"], &q["keep_failed"]), (&serde_json::json!("jobs"), &serde_json::json!("bump"), &serde_json::json!(true)));
    assert!(note(&app).starts_with("bound jobs") && note(&app).ends_with("for bump only"), "{}", note(&app));
    assert!(!format!("{:?}", app.admin.queues).contains(&url), "the URL itself is never shown");
    let shown = detail(&app);
    assert!(shown.contains("For: bump only") && shown.contains("a failed run's message kept") && shown.contains("no deployment uses it"), "{shown}");
    // A line its command wouldn't take is refused as the command refuses it.
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "other --nope");
    assert!(note(&app).contains("no option \"--nope\""), "{}", note(&app));
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Enter);
    assert!(app.admin.queues.is_empty(), "{}", note(&app));
}

/// The server's own secret, set from a name the server reads with where it may go, set again,
/// its history listed, then rolled back after saying what that would do; never a value shown.
#[test]
fn a_server_secret_is_set_from_the_environment_and_rolled_back() {
    let mut state = test_state();
    let sources = crate::secret_sources::Sources::of(&[("HOLON_SECRET_API_KEY", "first-api-value"), ("HOLON_SECRET_API_KEY_NEXT", "second-api-value")]);
    state.secrets = std::sync::Arc::new(crate::secrets::SecretStore::memory().with_sources(sources));
    let (state, _, tokens) = host_server(state, &[]);
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.admin_part(Part::Secrets);
    assert!(detail(&app).contains("Read from the environment"), "with none of its own, what the environment holds: {}", detail(&app));
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "api_key --from env:HOLON_SECRET_API_KEY --to api.stripe.com");
    assert!(note(&app).starts_with("api_key: version 1 set for the server"), "{}", note(&app));
    let shown = detail(&app);
    assert!(shown.contains("Source: env:HOLON_SECRET_API_KEY") && shown.contains("May go: only to https://api.stripe.com"), "{shown}");
    press(&mut app, KeyCode::Enter);
    answer(&mut app, "--from env:HOLON_SECRET_API_KEY_NEXT");
    assert!(note(&app).starts_with("api_key: version 2"), "{}", note(&app));
    let shown = detail(&app);
    assert!(shown.contains("v1") && shown.contains("v2") && shown.contains("(current)"), "its history: {shown}");
    press(&mut app, KeyCode::Char('R'));
    answer(&mut app, "1");
    let Some(Modal::Confirm { lines, .. }) = &app.modal else { panic!("no preview: {}", note(&app)) };
    assert!(lines.join(" ").contains("would make v1") && !lines.join(" ").contains("--dry-run"), "{lines:?}");
    press(&mut app, KeyCode::Enter);
    assert!(note(&app).contains("version 1 is current again, as version 3"), "{}", note(&app));
    // Where it may go, widened to anywhere.
    press(&mut app, KeyCode::Char('t'));
    answer(&mut app, "anywhere");
    assert!(note(&app).contains("wherever its flows' approvals show"), "{}", note(&app));
    let everything = format!("{:?} {:?}", app.admin.secrets, app.admin.pane);
    assert!(!everything.contains("first-api-value") && !everything.contains("second-api-value"), "never a value");
}

/// Who uses a folder: the deployment that reads it, and in amber the version an agent proposed
/// that would read it too, waiting for a person.
#[test]
fn a_resource_says_who_uses_it_and_what_a_waiting_version_would_add() {
    let reader = |flow: &str| format!("import {{ action, flow }} from \"holon\"\nimport inbox from \"folder:inbox\"\n\nflow(\"{flow}\")\n\nexport const peek = action(async (name) => inbox[name])\n");
    let mut state = test_state();
    for flow in ["reader", "later"] {
        crate::tests::stage_text(&state, &reader(flow));
    }
    let dir = std::env::temp_dir().join(format!("holon-used-by-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    state.folders = std::sync::Arc::new(crate::folders::Folders::memory());
    let (state, book, tokens) = host_server(state, &[]);
    book.add("claude", crate::accounts::Kind::Agent, "agent", "ada", "ada").unwrap();
    let claude = book.create_token("claude", "the test", "ada").unwrap().0;
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.admin_part(Part::Folders);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, &format!("inbox {}", dir.display()));
    assert_eq!(app.admin.folders.len(), 1, "{}", note(&app));
    app.go(Page::Editor);
    tab_with(&mut app, &reader("reader"));
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert!(app.client.as_ref().unwrap().deployment("reader").is_some(), "{}", note(&app));
    let agent = Client::new(Box::new(InProcess(state, Some(claude))));
    let plan = holon_author::agent::plan(&reader("later")).unwrap();
    let said = agent.post("/deploy", serde_json::json!({ "slug": "later", "unit": plan.unit, "document": plan.document, "why": "it reads the inbox too" }));
    assert!(said.as_ref().is_ok_and(|s| s["proposal"]["id"].is_u64()), "a proposal: {said:?}");
    app.refresh();
    app.go(Page::Admin);
    app.admin_part(Part::Folders);
    let rows = app.admin.pane.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
    assert!(rows.iter().any(|r| matches!(r, Row::Text(Tone::Plain, t) if t == "  reader: reads")), "{rows:?}");
    assert!(rows.iter().any(|r| matches!(r, Row::Text(Tone::Warn, t) if t.contains("later") && t.contains("waiting for a person, would add: reads"))), "amber: {rows:?}");
    screen(&mut app, 80, 24);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The policy part shows holon.toml's text, a line each, and what flows get; storage can't be
/// written into a policy file the server doesn't read from, and says so.
#[test]
fn the_policy_part_shows_its_text() {
    let (state, _, tokens) = host_server(test_state(), &[]);
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.admin_part(Part::Policy);
    let rows = app.admin.pane.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
    assert!(rows.iter().any(|r| matches!(r, Row::Code { n: 2, text, .. } if text == "allow_plain_http = true")), "{rows:?}");
    assert!(screen(&mut app, 140, 40).contains("allow_plain_http = true"));
    press(&mut app, KeyCode::Down);
    assert!(detail(&app).contains("plain http         allowed"), "{}", detail(&app));
    press(&mut app, KeyCode::Down);
    assert!(detail(&app).contains("covers every deployment"), "{}", detail(&app));
    app.admin_part(Part::Storage);
    press(&mut app, KeyCode::Enter);
    answer(&mut app, "90%");
    assert!(app.note.as_ref().is_some_and(|n| n.error) && note(&app).contains("no file"), "{}", note(&app));
}

/// Someone whose role holds resource.view alone sees the resources and the server's secrets,
/// and none of the keys that bind or unbind them; the storage and the policy say why not.
#[test]
fn someone_holding_resource_view_only_sees_the_resources_and_no_bind_keys() {
    let (state, book, _) = people_server(&[]);
    book.define_role(crate::bindings::RoleText { name: "looker".into(), about: None, allows: vec!["resource.view".into()], kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let lou = book.accept(&book.invite("lou", "looker", None, "ada").unwrap(), "correct horse battery").unwrap().0;
    let (mut app, calls) = admin_page_as(state, &lou);
    for part in [Part::Folders, Part::Databases, Part::Mail, Part::Feeds, Part::Queues, Part::Secrets] {
        app.admin_part(part);
        let shown = screen(&mut app, 140, 40);
        assert!(!shown.contains("It can't be shown here"), "{part:?}: {shown}");
        for key in ["bind", "unbind", "new secret", "roll back", "browse"] {
            assert!(!shown.contains(&format!("  {key}")), "{part:?}: no {key:?} key: {shown}");
        }
    }
    let before = calls.lock().unwrap().len();
    app.admin_part(Part::Queues);
    press(&mut app, KeyCode::Char('n'));
    assert!(app.modal.is_none() && note(&app).contains("resource.bind"), "{}", note(&app));
    assert_eq!(calls.lock().unwrap().len(), before, "sends nothing");
    for (part, takes) in [(Part::Storage, "host.view"), (Part::Policy, "policy.view")] {
        app.admin_part(part);
        let shown = screen(&mut app, 140, 40);
        assert!(shown.contains("It can't be shown here") && shown.contains(takes), "{part:?}: {shown}");
    }
}

/// b walks the server's folders: enter opens one, ← goes back to its parent, and n binds the
/// folder shown, its path in the dialog.
#[test]
fn a_folder_to_bind_is_found_by_walking_the_servers_folders() {
    let dir = std::env::temp_dir().join(format!("holon-walk-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Inbox Two")).unwrap();
    let (state, _, tokens) = people_server(&[]);
    let (mut app, _) = admin_page_as(state, &tokens["ada"]);
    app.admin_part(Part::Folders);
    press(&mut app, KeyCode::Char('b'));
    assert!(app.admin.browse.is_some(), "{}", note(&app));
    // Straight to the test's folder, as enter on each folder on the way would.
    app.admin.browse = Some(app.client.as_ref().unwrap().get(&format!("/admin/folders/browse?path={}", crate::mcp::seg(&dir.display().to_string()))).unwrap());
    app.admin.sel[Part::Folders as usize] = 0;
    let rows = super::admin::browse_rows(app.admin.browse.as_ref().unwrap());
    let at = rows.iter().position(|(name, _)| name == "Inbox Two").unwrap_or_else(|| panic!("{rows:?}"));
    app.admin_pick(List::Admin, at, false);
    press(&mut app, KeyCode::Enter);
    assert!(app.admin.browse.as_ref().is_some_and(|b| b["path"].as_str().is_some_and(|p| p.ends_with("Inbox Two"))), "{:?}", app.admin.browse);
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("Inbox Two") && shown.contains("bind this folder"), "{shown}");
    press(&mut app, KeyCode::Left);
    assert_eq!(app.admin.browse.as_ref().and_then(|b| b["path"].as_str()).map(|p| p.trim_end_matches(['/', '\\']).ends_with(&format!("holon-walk-{}", std::process::id()))), Some(true));
    app.admin_pick(List::Admin, at, false);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Char('n'));
    let Some(Modal::Input { value, .. }) = &app.modal else { panic!("no dialog: {}", note(&app)) };
    assert!(value.starts_with("inbox-two ") && value.ends_with("Inbox Two"), "{value}");
    assert!(app.admin.browse.is_none(), "the walk is over");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Step 6 against a live server: ada makes a group and a role, binds the role to the group on one
/// flow and to ana at one approval point from her page, then asks for one person's approval of
/// what gives more, and a binding waits for bob in Approvals › Access changes; ana, no admin,
/// makes a key for her agent on Your agents. Each page it passes is written to `HOLON_SCREENS`.
#[test]
#[ignore = "runs against a live server at HOLON_LIVE"]
fn live_access() {
    let server = std::env::var("HOLON_LIVE").expect("HOLON_LIVE: the server's address");
    let out = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write the pages");
    let pw = "correct horse battery";
    let open = |token: Option<&str>| Client::new(Box::new(crate::mcp::Http::new(&server, token)));
    let ok = |said: Result<serde_json::Value, String>, what: &str| said.unwrap_or_else(|e| panic!("{what}: {e}"));
    let ada = ok(open(None).post("/auth/setup", serde_json::json!({ "code": "HOLON-LIVE", "name": "ada", "password": pw })), "setup")["token"].as_str().unwrap().to_string();
    let as_ada = open(Some(&ada));
    let join = |name: &str, role: &str| {
        let invite = ok(as_ada.post("/people", serde_json::json!({ "name": name, "kind": "person", "role": role })), name)["invite"].as_str().unwrap().to_string();
        ok(open(None).post("/auth/invite", serde_json::json!({ "token": invite, "password": pw })), "joins")["token"].as_str().unwrap().to_string()
    };
    let bob = join("bob", "admin");
    let ana = join("ana", "editor");
    ok(as_ada.post("/people", serde_json::json!({ "name": "claude", "kind": "agent", "role": "agent", "owner": "ana" })), "claude");
    let mut pages = String::new();
    let mut keep = |name: &str, app: &mut App| {
        pages += &format!("\n===== {name} =====\n{}\nnote: {}\n", screen(app, 140, 40), note(app));
        std::fs::write(&out, &pages).unwrap();
    };
    let admin_page = |token: &str| {
        let mut app = App::new(None);
        app.attach(open(Some(token)));
        app.refresh();
        press(&mut app, KeyCode::Char('7'));
        app
    };

    let mut app = admin_page(&ada);
    app.admin_part(Part::Groups);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payments ana --about the payments team --why for the pilot");
    press(&mut app, KeyCode::Char('+'));
    answer(&mut app, "bob");
    keep("Groups: payments made, bob added", &mut app);
    assert_eq!(app.admin.groups[0]["members"], serde_json::json!(["ana", "bob"]), "{}", note(&app));

    app.admin_part(Part::Roles);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payout-approver run.view,run.decide --kinds person --about approves payouts");
    let at = app.admin.roles.iter().position(|r| r["name"] == "payout-approver").unwrap_or_else(|| panic!("{}", note(&app)));
    app.admin_pick(List::Admin, at, false);
    keep("Roles: payout-approver defined", &mut app);

    app.admin_part(Part::Bindings);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "payout-approver --group payments --flow payout --why the payout team decides");
    keep("Bindings: to the group, on one flow", &mut app);
    assert!(note(&app).starts_with("bound payout-approver to the group payments"), "{}", note(&app));

    // From ana's own page: the bind dialog comes with her name in it.
    app.admin_part(Part::People);
    let at = app.admin.users.iter().position(|u| u["name"] == "ana").unwrap();
    app.admin_pick(List::Admin, at, false);
    press(&mut app, KeyCode::Char('b'));
    keep("People: ana's bind dialog", &mut app);
    typed(&mut app, "payout-approver --approval payout:ok --until 7d --why covering for bob\n");
    keep("People: ana bound at one approval point", &mut app);
    assert!(note(&app).contains("for \"payout\"'s approval \"ok\""), "{}", note(&app));

    app.admin_part(Part::History);
    press(&mut app, KeyCode::Enter);
    answer(&mut app, "--changes 1 --why two people see each change");
    keep("Access history: one approval asked", &mut app);
    assert!(note(&app).starts_with("approval settings changed: --changes 1"), "{}", note(&app));
    press(&mut app, KeyCode::Down);
    keep("Access history: the newest version", &mut app);

    app.admin_part(Part::Bindings);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "admin --to ana --until 7d --why covering for bob");
    keep("Bindings: one that gives more waits", &mut app);
    assert!(note(&app).contains("Approvals › Access changes decides it"), "{}", note(&app));

    // bob sees it waiting, in Approvals › Access changes.
    let mut app = App::new(None);
    app.attach(open(Some(&bob)));
    app.refresh();
    press(&mut app, KeyCode::Char('2'));
    app.approvals_part = super::approvals::Part::Access;
    keep("bob: Approvals › Access changes", &mut app);
    assert!(!app.changes_waiting().is_empty(), "nothing waits for bob");

    // ana, an editor, makes a key for her agent.
    let mut app = admin_page(&ana);
    app.admin_part(Part::Yours);
    keep("ana: Your agents", &mut app);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "nightly report");
    assert!(matches!(&app.modal, Some(Modal::Lines { title, .. }) if title == "claude: an API key"), "{}", note(&app));
    press(&mut app, KeyCode::Esc);
    keep("ana: a key made", &mut app);
    assert_eq!(app.admin.owned[0]["tokens"].as_array().map(Vec::len), Some(1));
    app.admin_part(Part::Roles);
    keep("ana: Roles, as an editor sees them", &mut app);
}

/// Step 7 against a live server: ada walks to a folder and binds it, deploys a flow that reads it,
/// and an agent's version that would read it too waits, shown in amber under the folder; a server
/// secret is typed in, typed again, and rolled back after the dry run says what it would do; a
/// queue's line is sent as holon queue add takes it; the policy and storage say what they can.
/// Each page it passes is written to `HOLON_SCREENS`.
#[test]
#[ignore = "drives a live server: HOLON_LIVE=http://127.0.0.1:7591 HOLON_SCREENS=pages.txt"]
fn live_host() {
    let server = std::env::var("HOLON_LIVE").expect("HOLON_LIVE: the server's address");
    let out = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write the pages");
    let open = |token: Option<&str>| Client::new(Box::new(crate::mcp::Http::new(&server, token)));
    let ok = |said: Result<serde_json::Value, String>, what: &str| said.unwrap_or_else(|e| panic!("{what}: {e}"));
    let ada = ok(open(None).post("/auth/setup", serde_json::json!({ "code": "HOLON-LIVE", "name": "ada", "password": "correct horse battery" })), "setup")["token"].as_str().unwrap().to_string();
    let as_ada = open(Some(&ada));
    ok(as_ada.post("/people", serde_json::json!({ "name": "claude", "kind": "agent", "role": "agent", "owner": "ada" })), "claude");
    let claude = ok(as_ada.post("/people/claude/tokens", serde_json::json!({ "label": "the live run" })), "claude's key")["token"].as_str().unwrap().to_string();
    let mut pages = String::new();
    let mut keep = |name: &str, app: &mut App| {
        pages += &format!("\n===== {name} =====\n{}\nnote: {}\n", screen(app, 140, 40), note(app));
        std::fs::write(&out, &pages).unwrap();
    };
    let mut app = App::new(None);
    app.attach(open(Some(&ada)));
    app.refresh();
    press(&mut app, KeyCode::Char('7'));

    // A folder, found by walking the server's folders.
    let root = std::env::temp_dir().join(format!("holon-live-{}", std::process::id()));
    std::fs::create_dir_all(root.join("inbox")).unwrap();
    app.admin_part(Part::Folders);
    press(&mut app, KeyCode::Char('b'));
    keep("Folders: the walk starts", &mut app);
    // Straight to the run's folder, as enter on each folder on the way would.
    app.admin.browse = Some(ok(as_ada.get(&format!("/admin/folders/browse?path={}", crate::mcp::seg(&root.display().to_string()))), "browse"));
    let at = super::admin::browse_rows(app.admin.browse.as_ref().unwrap()).iter().position(|(name, _)| name == "inbox").expect("inbox");
    app.admin_pick(List::Admin, at, false);
    press(&mut app, KeyCode::Enter);
    keep("Folders: inside inbox", &mut app);
    press(&mut app, KeyCode::Char('n'));
    keep("Folders: the bind dialog, its path in it", &mut app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.admin.folders.len(), 1, "{}", note(&app));

    // A flow that reads it, deployed; an agent's version that would read it too, waiting.
    let reader = |flow: &str| format!("import {{ action, flow }} from \"holon\"\nimport inbox from \"folder:inbox\"\n\nflow(\"{flow}\")\n\nexport const peek = action(async (name) => inbox[name])\n");
    app.go(Page::Editor);
    tab_with(&mut app, &reader("reader"));
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert!(app.client.as_ref().unwrap().deployment("reader").is_some(), "{}", note(&app));
    let plan = holon_author::agent::plan(&reader("later")).unwrap();
    let said = open(Some(&claude)).post("/deploy", serde_json::json!({ "slug": "later", "unit": plan.unit, "document": plan.document, "why": "it reads the inbox too" }));
    assert!(said.as_ref().is_ok_and(|s| s["proposal"]["id"].is_u64()), "a proposal: {said:?}");
    app.refresh();
    app.go(Page::Admin);
    app.admin_part(Part::Folders);
    keep("Folders: inbox, used by reader, and later waiting", &mut app);
    let rows = app.admin.pane.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
    assert!(rows.iter().any(|r| matches!(r, Row::Text(Tone::Warn, t) if t.contains("later"))), "amber: {rows:?}");

    // The server's own secret, typed in twice, then rolled back.
    app.admin_part(Part::Secrets);
    keep("Secrets: none of its own yet", &mut app);
    press(&mut app, KeyCode::Char('n'));
    answer(&mut app, "api_key --to api.stripe.com");
    keep("Secrets: its value asked, masked", &mut app);
    answer(&mut app, "first-live-value");
    assert!(note(&app).starts_with("api_key: version 1"), "{}", note(&app));
    press(&mut app, KeyCode::Enter);
    answer(&mut app, "");
    answer(&mut app, "second-live-value");
    assert!(note(&app).starts_with("api_key: version 2"), "{}", note(&app));
    keep("Secrets: api_key, version 2", &mut app);
    press(&mut app, KeyCode::Char('R'));
    answer(&mut app, "1");
    keep("Secrets: what rolling back would do", &mut app);
    press(&mut app, KeyCode::Enter);
    keep("Secrets: rolled back", &mut app);
    assert!(note(&app).contains("version 1 is current again"), "{}", note(&app));
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(!written.contains("first-live-value") && !written.contains("second-live-value"), "never a value");

    // A queue, as holon queue add takes it: what the live server says to it.
    app.admin_part(Part::Queues);
    press(&mut app, KeyCode::Char('n'));
    keep("Queues: the bind dialog", &mut app);
    answer(&mut app, "jobs --url env:HOLON_SECRET_JOBS_URL --for reader");
    keep("Queues: what the server says", &mut app);

    for (part, name) in [(Part::Mail, "Mail"), (Part::Feeds, "Feeds"), (Part::Storage, "Storage"), (Part::Policy, "Policy")] {
        app.admin_part(part);
        keep(name, &mut app);
    }
    press(&mut app, KeyCode::Down);
    keep("Policy: what flows get", &mut app);
    press(&mut app, KeyCode::Down);
    keep("Policy: held off", &mut app);
    let _ = std::fs::remove_dir_all(&root);
}

/// The right pane's text, a line each.
fn pane_text(app: &App) -> String {
    app.flow
        .pane
        .as_ref()
        .map(|(_, rows)| {
            rows.iter()
                .map(|r| match r {
                    Row::Text(_, t) => t.clone(),
                    Row::Field(k, v, _) => format!("{k}: {v}"),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The flow page's Runs on the run `seq`.
fn on_run(app: &mut App, slug: &str, seq: u64) {
    app.open_flow(slug);
    app.flow_section(Section::Runs);
    let d = app.detail.as_ref().unwrap();
    let at = d.shown_runs("").iter().position(|&i| d.runs[i]["seq"].as_u64() == Some(seq)).unwrap_or_else(|| panic!("no run {seq}"));
    app.flow.follow = false;
    app.flow_pick(List::Section, at);
}

/// A run decided by two people says who decided it, each in a line, as the web's run page does:
/// ada approves with why, then bob, and the run that goes on says both, and what let them.
#[test]
fn a_run_decided_in_two_steps_shows_both_decisions() {
    let (_, open) = waiting_payouts();
    let (mut ada, _) = open("ada");
    let first = ada.runs_waiting()[0]["run_id"].as_str().unwrap().to_string();
    press(&mut ada, KeyCode::Char('a'));
    typed(&mut ada, "fine by me\n");
    let (mut bob, _) = open("bob");
    bob.run_sel = bob.runs_waiting().iter().position(|r| r["run_id"] == first.as_str()).unwrap();
    press(&mut bob, KeyCode::Char('a'));
    press(&mut bob, KeyCode::Enter);
    assert!(note(&bob).contains("goes on"), "{}", note(&bob));
    ada.refresh();
    ada.open_flow("payouts");
    let d = ada.detail.as_ref().unwrap();
    let went_on = d.runs.iter().find(|r| r["run_id"] == first.as_str() && r["events"].is_object()).unwrap_or_else(|| panic!("the part that went on: {:?}", d.runs));
    let seq = went_on["seq"].as_u64().unwrap();
    on_run(&mut ada, "payouts", seq);
    let shown = pane_text(&ada);
    assert!(shown.contains("Decided") && shown.contains("\"payout\": approved by ada and bob"), "{shown}");
    let decided: Vec<&str> = shown.lines().filter(|l| l.contains(" approved")).collect();
    assert!(decided.iter().any(|l| l.contains("ada approved: fine by me")), "{decided:?}");
    assert!(decided.iter().any(|l| l.contains("bob approved")), "{decided:?}");
    assert!(decided.iter().all(|l| !l.contains("approved by") || l.contains("\"payout\"")), "{decided:?}");
    screen(&mut ada, 140, 40);
}

/// A run that read the server's secret and sent it in a request names the secret, its version,
/// and the host it went to; never its value, anywhere on screen.
#[test]
fn a_run_that_sent_a_secret_names_the_host_and_never_the_value() {
    let (host, _) = crate::tests::counting_server();
    let state = crate::tests::claims_state("");
    let bind = serde_json::json!({ "value": "srv_live_shared_77", "to": [host] });
    assert_eq!(route(&state, &req("POST", "/admin/secrets/api_key", serde_json::to_vec(&bind).unwrap())).status_code, 200);
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport api_key from \"secret:api_key\"\nimport api from \"host:{host}\"\n\nflow(\"sends\", {{ port: 18193 }})\n\nroute.get(\"/go\", async () => {{\n  const called = await api.get(\"/api\", {{ headers: {{ authorization: `Bearer ${{api_key}}` }} }})\n  return {{ status: called.status }}\n}})\n"
    );
    let slug = crate::tests::deploy_text_to(&state, &text, &[&host]);
    let (status, ..) = crate::tests::data_call(&state, &slug, "GET", "/go", "");
    assert_eq!(status, 200);
    let state: &'static AppState = Box::leak(Box::new(state));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state, None))));
    app.refresh();
    let seq = app.client.as_ref().unwrap().detail(&slug).unwrap().runs.last().unwrap()["seq"].as_u64().unwrap();
    on_run(&mut app, &slug, seq);
    let shown = pane_text(&app);
    assert!(shown.contains("Secrets it read"), "{shown}");
    let line = shown.lines().find(|l| l.starts_with("api_key ")).unwrap_or_else(|| panic!("{shown}"));
    assert!(line.contains("v1 #") && line.contains("the server's own") && line.ends_with(&format!("sent to {host}")), "{line}");
    assert!(shown.contains("Witnessed: no witness set"), "{shown}");
    let drawn = screen(&mut app, 160, 60);
    assert!(!shown.contains("srv_live") && !drawn.contains("srv_live"), "never the value");
}

/// A collection of 250 items is shown a page at a time, to its end and back; one item's history
/// is the runs that changed it.
#[test]
fn a_collection_of_250_items_pages_to_the_end() {
    let many = "import { action, flow } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"many\")\n\nexport const add = action((input) => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  notes[`k${n}`] = n\n})\n";
    let (mut app, state) = app();
    tab_with(&mut app, many);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    for _ in 0..250 {
        assert_eq!(route(state, &req("POST", "/trigger/many/add", b"{}".to_vec())).status_code, 200);
    }
    app.refresh();
    app.open_flow("many");
    app.flow_section(Section::State);
    let at = app.flow.cells.iter().position(|c| c.collection && c.name == "notes").unwrap_or_else(|| panic!("{:?}", app.flow.cells));
    app.flow_pick(List::Section, at);
    let page = |app: &App| pane_text(app).lines().next().unwrap_or_default().to_string();
    assert_eq!(page(&app), "the collection notes: items 1 to 100");
    let hints = screen(&mut app, 160, 40);
    assert!(hints.contains("pages") && hints.contains("an item's history"), "{hints}");
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(page(&app), "the collection notes: items 101 to 200");
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(page(&app), "the collection notes: items 201 to 250");
    assert!(pane_text(&app).contains("the last of them"), "{}", pane_text(&app));
    let rows = pane_text(&app).lines().filter(|l| l.starts_with('k')).count();
    assert_eq!(rows, 50, "the last 50");
    press(&mut app, KeyCode::Char(']'));
    assert!(note(&app).contains("the last page"), "{}", note(&app));
    press(&mut app, KeyCode::Char('['));
    assert_eq!(page(&app), "the collection notes: items 101 to 200");
    press(&mut app, KeyCode::Char('i'));
    answer(&mut app, "k7");
    let Some(Modal::Lines { title, lines, .. }) = &app.modal else { panic!("no history: {}", note(&app)) };
    assert!(title.contains("k7"), "{title}");
    assert!(lines.iter().any(|(_, l)| l.contains("#6")), "run 6 put it: {lines:?}");
}

/// Check now, from the sidebar, reads every run log again: a line changed outside holon is a
/// break, and enter opens its run. The overview reads the host in one summary, not run by run.
#[test]
fn check_now_on_a_log_edited_outside_holon_reports_the_break() {
    let at = crate::tests::unique_temp_path("check-now");
    let state: &'static AppState = Box::leak(Box::new(crate::tests::test_state_at(crate::paths::Place::parse(&at))));
    let calls = std::sync::Arc::new(crate::sync::Mutex::new(vec![]));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(Recorded(InProcess(state, None), std::sync::Arc::clone(&calls)))));
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    for _ in 0..3 {
        assert_eq!(route(state, &req("POST", "/trigger/bump/bump", b"{}".to_vec())).status_code, 200);
    }
    calls.lock().unwrap().clear();
    app.refresh();
    app.go(Page::Overview);
    let read: Vec<String> = calls.lock().unwrap().clone();
    assert!(read.iter().any(|c| c.contains("/summary?hours=24")), "{read:?}");
    assert!(!read.iter().any(|c| c.contains("/deployments/bump/log?")), "no read of its own: {read:?}");
    let shown = screen(&mut app, 160, 40);
    assert!(shown.contains("check now") && shown.contains("3 runs"), "{shown}");

    // Run 1's line changed: run 2's no longer names it.
    let path = crate::recorder::file_for(&state.deployments_path, "bump").unwrap();
    let mut lines = crate::at_rest::testing::open_lines(&path);
    assert_eq!(lines.len(), 3);
    lines[1] = lines[1].replacen("\"session\":", "\"session\" :", 1);
    crate::at_rest::testing::write_lines(&path, &lines);
    press(&mut app, KeyCode::Char('C'));
    let Some(Modal::Checked { lines, breaks, .. }) = &app.modal else { panic!("no check: {}", note(&app)) };
    assert_eq!(breaks, &[("bump".to_string(), 2)], "{lines:?}");
    assert!(lines.iter().any(|(_, l)| l.starts_with("3 runs read")), "{lines:?}");
    let shown = screen(&mut app, 140, 40);
    assert!(shown.contains("bump, run #2") && shown.contains("open the run"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Flow);
    let d = app.detail.as_ref().unwrap();
    assert_eq!(d.shown_runs("").get(app.flow.sel()).map(|&i| d.runs[i]["seq"].clone()), Some(serde_json::json!(2)), "{}", note(&app));
    let footer = screen(&mut app, 140, 40);
    assert!(footer.contains("1 found"), "the sidebar says it: {footer}");
    let _ = std::fs::remove_dir_all(&at);
    let _ = std::fs::remove_file(&at);
}

/// Step 8 against a live server: a run decided by ada and bob says both; a run that sent the
/// server's secret names the host and never the value; a collection of 250 items pages to its end;
/// the overview reads one summary, and the run logs are checked from the sidebar. Each page it
/// passes is written to `HOLON_SCREENS`.
#[test]
#[ignore = "drives a live server: HOLON_LIVE=http://127.0.0.1:7591 HOLON_SCREENS=pages.txt"]
fn live_record() {
    let server = std::env::var("HOLON_LIVE").expect("HOLON_LIVE: the server's address");
    let out = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write the pages");
    let pw = "correct horse battery";
    let open = |token: Option<&str>| Client::new(Box::new(crate::mcp::Http::new(&server, token)));
    let ok = |said: Result<serde_json::Value, String>, what: &str| said.unwrap_or_else(|e| panic!("{what}: {e}"));
    let ada = ok(open(None).post("/auth/setup", serde_json::json!({ "code": "HOLON-LIVE", "name": "ada", "password": pw })), "setup")["token"].as_str().unwrap().to_string();
    let as_ada = open(Some(&ada));
    let invite = ok(as_ada.post("/people", serde_json::json!({ "name": "bob", "kind": "person", "role": "admin" })), "bob")["invite"].as_str().unwrap().to_string();
    let bob = ok(open(None).post("/auth/invite", serde_json::json!({ "token": invite, "password": pw })), "joins")["token"].as_str().unwrap().to_string();
    // cy starts the run: whoever started it can't approve it.
    let invite = ok(as_ada.post("/people", serde_json::json!({ "name": "cy", "kind": "person", "role": "admin" })), "cy")["invite"].as_str().unwrap().to_string();
    let cy = ok(open(None).post("/auth/invite", serde_json::json!({ "token": invite, "password": pw })), "joins")["token"].as_str().unwrap().to_string();
    let mut pages = String::new();
    let mut keep = |name: &str, app: &mut App| {
        pages += &format!("\n===== {name} =====\n{}\nnote: {}\n", screen(app, 140, 40), note(app));
        std::fs::write(&out, &pages).unwrap();
    };
    let signed_in = |token: &str| {
        let mut app = App::new(None);
        app.attach(open(Some(token)));
        app.refresh();
        app
    };
    let mut app = signed_in(&ada);
    let deploy = |app: &mut App, text: &str| {
        tab_with(app, text);
        ctrl(app, 'd');
        press(app, KeyCode::Char('t'));
        press(app, KeyCode::Enter);
        app.refresh();
    };

    // A two-person approval, decided by ada and then bob.
    let payouts = "import { action, approval, flow } from \"holon\"\n\nflow(\"payouts\")\n\nexport const pay = action(async (order) => {\n  const decision = await approval(\"payout\", { expires: \"1h\", people: 2 })\n  return decision.approved\n})\n";
    deploy(&mut app, payouts);
    assert!(app.client.as_ref().unwrap().deployment("payouts").is_some(), "{}", note(&app));
    ok(open(Some(&cy)).post("/trigger/payouts/pay", serde_json::json!({ "amount": 10 })), "pay");
    app.refresh();
    app.go(Page::Approvals);
    press(&mut app, KeyCode::Tab);
    keep("Approvals › Runs waiting, as ada", &mut app);
    press(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.modal, Some(Modal::Input { .. })), "no dialog: {}", note(&app));
    typed(&mut app, "fine by me\n");
    keep("ada approved", &mut app);
    assert!(note(&app).contains("1 of 2"), "{}", note(&app));
    let mut by_bob = signed_in(&bob);
    by_bob.go(Page::Approvals);
    press(&mut by_bob, KeyCode::Tab);
    press(&mut by_bob, KeyCode::Char('a'));
    press(&mut by_bob, KeyCode::Enter);
    assert!(note(&by_bob).contains("goes on"), "{}", note(&by_bob));
    app.refresh();
    app.open_flow("payouts");
    let seq = app.detail.as_ref().unwrap().runs.iter().rev().find(|r| r["events"].is_object()).and_then(|r| r["seq"].as_u64()).expect("the part that went on");
    on_run(&mut app, "payouts", seq);
    keep("payouts: the run that went on, decided by two", &mut app);
    assert!(pane_text(&app).contains("approved by ada and bob"), "{}", pane_text(&app));

    // The server's secret, sent to a host it may go to.
    let (host, _) = crate::tests::counting_server();
    ok(as_ada.post("/admin/secrets/api_key", serde_json::json!({ "value": "srv_live_shared_77", "to": [host] })), "the secret");
    let sends = format!(
        "import {{ flow, route }} from \"holon\"\nimport api_key from \"secret:api_key\"\nimport api from \"host:{host}\"\n\nflow(\"sends\", {{ port: 18194 }})\n\nroute.get(\"/go\", async () => {{\n  const called = await api.get(\"/api\", {{ headers: {{ authorization: `Bearer ${{api_key}}` }} }})\n  return {{ status: called.status }}\n}})\n"
    );
    deploy(&mut app, &sends);
    keep("sends: deployed", &mut app);
    assert!(app.client.as_ref().unwrap().deployment("sends").is_some(), "{}", note(&app));
    ok(as_ada.request("sends", "GET", "/go", ""), "go");
    app.refresh();
    let seq = app.client.as_ref().unwrap().detail("sends").unwrap().runs.last().unwrap()["seq"].as_u64().unwrap();
    on_run(&mut app, "sends", seq);
    keep("sends: the run that sent the secret", &mut app);
    let shown = pane_text(&app);
    assert!(shown.lines().any(|l| l.starts_with("api_key ") && l.contains(&host)), "{shown}");
    press(&mut app, KeyCode::Char('O'));
    keep("sends: this run exported", &mut app);
    assert!(note(&app).contains(&format!("sends-run-{seq}.json")), "{}", note(&app));
    let _ = std::fs::remove_file(format!("sends-run-{seq}.json"));

    // 250 items, a page at a time.
    let many = "import { action, flow } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"many\")\n\nexport const add = action((input) => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  notes[`k${n}`] = n\n})\n";
    deploy(&mut app, many);
    for _ in 0..250 {
        ok(as_ada.post("/trigger/many/add", serde_json::json!({})), "add");
    }
    app.refresh();
    app.open_flow("many");
    keep("many: Status, connected to nothing", &mut app);
    app.flow_section(Section::State);
    let at = app.flow.cells.iter().position(|c| c.collection).expect("notes");
    app.flow_pick(List::Section, at);
    keep("many: notes, items 1 to 100", &mut app);
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Char(']'));
    keep("many: notes, items 201 to 250", &mut app);
    assert!(pane_text(&app).starts_with("the collection notes: items 201 to 250"), "{}", pane_text(&app));
    press(&mut app, KeyCode::Char('i'));
    answer(&mut app, "k250");
    keep("many: k250's history", &mut app);
    press(&mut app, KeyCode::Esc);

    // The overview, from one summary; the run logs checked from the sidebar.
    app.go(Page::Overview);
    keep("Overview", &mut app);
    press(&mut app, KeyCode::Char('C'));
    keep("The run logs, checked now", &mut app);
    assert!(matches!(&app.modal, Some(Modal::Checked { breaks, .. }) if breaks.is_empty()), "{}", note(&app));
    press(&mut app, KeyCode::Esc);
    keep("Overview, after the check", &mut app);
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(!written.contains("srv_live"), "never the value");
}

/// Step 9 against a live server (`holon serve`, set up with `HOLON_SETUP_CODE=HOLON-LIVE`): the
/// palette opens a run by its number and a deploy waiting by its name; `?` lists the keys page by
/// page; a saved flow is deleted from Flows, asked first, and the server no longer has it. Each
/// page it passes is written to `HOLON_SCREENS`.
#[test]
#[ignore = "drives a live server: HOLON_LIVE=http://127.0.0.1:7591 HOLON_SCREENS=pages.txt"]
fn live_small() {
    let server = std::env::var("HOLON_LIVE").expect("HOLON_LIVE: the server's address");
    let out = std::env::var("HOLON_SCREENS").expect("HOLON_SCREENS: where to write the pages");
    let open = |token: Option<&str>| Client::new(Box::new(crate::mcp::Http::new(&server, token)));
    let ok = |said: Result<serde_json::Value, String>, what: &str| said.unwrap_or_else(|e| panic!("{what}: {e}"));
    let ada = ok(open(None).post("/auth/setup", serde_json::json!({ "code": "HOLON-LIVE", "name": "ada", "password": "correct horse battery" })), "setup")["token"].as_str().unwrap().to_string();
    let as_ada = open(Some(&ada));
    let mut pages = String::new();
    let mut keep = |name: &str, app: &mut App| {
        pages += &format!("\n===== {name} =====\n{}\nnote: {}\n", screen(app, 140, 40), note(app));
        std::fs::write(&out, &pages).unwrap();
    };
    let mut app = App::new(None);
    app.attach(open(Some(&ada)));
    app.refresh();
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    for _ in 0..3 {
        ok(as_ada.post("/trigger/bump/bump", serde_json::json!({})), "bump");
    }
    let plan = holon_author::agent::plan(BEATS).unwrap();
    as_ada.save("beats", &plan, BEATS, None).unwrap();
    app.refresh();
    press(&mut app, KeyCode::Esc);

    ctrl(&mut app, 'p');
    keep("ctrl+p, nothing typed", &mut app);
    typed(&mut app, "#2");
    keep("ctrl+p #2", &mut app);
    press(&mut app, KeyCode::Enter);
    keep("bump, run #2", &mut app);
    let d = app.detail.as_ref().unwrap();
    assert_eq!(d.shown_runs("").get(app.flow.sel()).and_then(|&i| d.runs[i]["seq"].as_u64()), Some(2), "{}", note(&app));

    ctrl(&mut app, 'p');
    typed(&mut app, "beats waits");
    press(&mut app, KeyCode::Enter);
    keep("Approvals, beats picked", &mut app);
    assert_eq!(app.waiting()[app.sel[1]].0, "beats");

    press(&mut app, KeyCode::Char('?'));
    keep("the keys", &mut app);
    for _ in 0..3 {
        press(&mut app, KeyCode::PageDown);
    }
    keep("the keys, further down", &mut app);
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('3'));
    app.sel[2] = app.flow_names().iter().position(|n| n == "beats").unwrap();
    press(&mut app, KeyCode::Delete);
    keep("Flows: delete beats?", &mut app);
    press(&mut app, KeyCode::Enter);
    keep("Flows: beats deleted", &mut app);
    assert!(note(&app).contains("deleted the saved flow \"beats\""), "{}", note(&app));
    let gone = as_ada.get("/flows/beats");
    assert!(gone.as_ref().is_err_and(|e| e.contains("no flow saved")), "{gone:?}");
}
