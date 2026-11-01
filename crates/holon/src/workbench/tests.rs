//! `holon editor` driven by its keys against the real router, in-process, and drawn to a test
//! screen: what a person sees and does there.

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;

use super::app::{App, Modal, Page};
use super::client::Client;
use super::ui;
use crate::mcp::{Api, Body};
use crate::tests::{body_bytes, req, test_state};
use crate::{route, AppState};

/// The router in-process, as the editor's server.
struct InProcess(&'static AppState);

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
        let response = route(self.0, &req(method, path, bytes));
        Ok((response.status_code, body_bytes(response)))
    }
}

fn app() -> (App, &'static AppState) {
    let state: &'static AppState = Box::leak(Box::new(test_state()));
    let mut app = App::new(None);
    app.attach(Client::new(Box::new(InProcess(state))));
    (app, state)
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

const BEATS: &str = "flow beats\nevery 5s\n\nstate.beats = (state.beats ?? 0) + 1\n";
/// A flow run by hand.
const BUMP: &str = "flow bump\n\naction bump (input) {\n  n = (state.n ?? 0) + 1\n  state.n = n\n  respond n\n}\n";

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
    press(&mut app, KeyCode::Char('2'));
    let shown = screen(&mut app, 120, 30);
    assert!(shown.contains("beats") && shown.contains("first time"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Editor);
    assert!(matches!(app.modal, Some(Modal::Approve { .. })), "opens it and asks to approve it");
    press(&mut app, KeyCode::Enter);
    assert!(app.waiting().is_empty(), "{:?}", app.waiting());
}

#[test]
fn a_flows_page_shows_its_runs_and_a_run_explained() {
    let (mut app, state) = app();
    tab_with(&mut app, BUMP);
    ctrl(&mut app, 'd');
    press(&mut app, KeyCode::Enter);
    let ran = route(state, &req("POST", "/trigger/bump/bump", b"{}".to_vec()));
    let status = ran.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(ran)));
    app.refresh();

    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.page, Page::Flow);
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("It may") && shown.contains("state.n = n"), "{shown}");
    press(&mut app, KeyCode::Right);
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("run 0:"), "{shown}");
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Explain { lines, .. }) = &app.modal else { panic!("{:?}", app.note.as_ref().map(|n| &n.text)) };
    assert!(lines.iter().any(|l| l.contains("⇒")), "{lines:#?}");
    press(&mut app, KeyCode::Esc);

    // In the editor, the same run over its text, a step at a time.
    press(&mut app, KeyCode::Char('5'));
    ctrl(&mut app, 'r');
    let runs = app.tabs[0].runs.as_ref().expect("view runs");
    assert!(!runs.noted().is_empty());
    let shown = screen(&mut app, 120, 40);
    assert!(shown.contains("View runs") && shown.contains("step 1 of"), "{shown}");
}

#[test]
fn every_page_draws_narrow_and_wide_and_without_a_server() {
    let (mut app, _) = app();
    tab_with(&mut app, BEATS);
    for page in [Page::Overview, Page::Approvals, Page::Flows, Page::Editor, Page::Library] {
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
    assert!(shown.contains("hello-api (sample)") && shown.contains("route GET /hello/:name"), "{shown}");
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
