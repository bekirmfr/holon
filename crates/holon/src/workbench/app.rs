//! What `holon editor` holds and what each key and click does: the pages, the open flow, the
//! tabs, and the dialog, palette or prompt in front of them. Drawing it is ui's; a flow's page is
//! flow.rs's.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use holon_author::editing::{self, Class, Mistake};
use holon_author::lang::ast::Pos;
use holon_author::{agent, samples};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use serde_json::Value;

use super::client::{self, Client, Detail, Point};
use super::flow::{CallForm, FlowView};
use super::palette;
use super::text::Buffer;
use crate::deploycmd::Staged;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Overview,
    Approvals,
    Flows,
    /// One deployment: its dashboard, runs, history, state and access.
    Flow,
    Editor,
    Library,
    /// The server's people, keys, folders, databases, secrets and settings (admin.rs).
    Admin,
}

/// The pages, in the sidebar's order: each one's key is its place.
pub(crate) const PAGES: [(Page, &str); 7] =
    [(Page::Overview, "Overview"), (Page::Approvals, "Approvals"), (Page::Flows, "Flows"), (Page::Flow, "Flow"), (Page::Editor, "Editor"), (Page::Library, "Library"), (Page::Admin, "Admin")];

/// What the text says, checked: its mistakes, its colours by line, and what it deploys as.
pub(crate) struct Check {
    pub(crate) mistakes: Vec<Mistake>,
    /// By line (from 0): (from, to, class), in characters from 0.
    pub(crate) colours: Vec<Vec<(usize, usize, Class)>>,
    pub(crate) plan: Result<agent::Plan, String>,
}

impl Check {
    pub(crate) fn of(text: &str) -> Check {
        let mut colours: Vec<Vec<(usize, usize, Class)>> = vec![vec![]; text.split('\n').count()];
        for h in editing::highlight(text) {
            if let Some(line) = colours.get_mut(h.span.start.line.saturating_sub(1) as usize) {
                let to = if h.span.end.line == h.span.start.line { h.span.end.col } else { u32::MAX };
                line.push((h.span.start.col.saturating_sub(1) as usize, to.saturating_sub(1) as usize, h.class));
            }
        }
        Check { mistakes: editing::mistakes(text), colours, plan: agent::plan(text) }
    }
}

/// A tab's "View runs": the deployment's runs, and one of them over its version's text.
pub(crate) struct Runs {
    pub(crate) slug: String,
    /// Newest first.
    pub(crate) entries: Vec<Value>,
    pub(crate) sel: usize,
    pub(crate) view: Option<Result<agent::RunView, String>>,
    /// Which noted line the view is at, among those with notes.
    pub(crate) step: usize,
}

impl Runs {
    /// Its timeline: the lines (1-based) its statements ran on, in the order they ran; for a run
    /// that recorded no timing, the lines with a value, in order.
    pub(crate) fn steps(&self) -> Vec<u32> {
        match &self.view {
            Some(Ok(v)) if !v.steps.is_empty() => v.steps.iter().map(|s| s.line).collect(),
            Some(Ok(v)) => v.notes.iter().filter(|(_, ns)| ns.iter().any(|n| !n.starts_with("⇒ nothing"))).map(|(l, _)| *l).collect(),
            _ => vec![],
        }
    }

    /// The line of the step picked.
    pub(crate) fn at(&self) -> Option<u32> {
        self.steps().get(self.step).copied()
    }

    /// Go `by` steps on, round the ends.
    pub(crate) fn go(&mut self, by: isize) {
        let n = self.steps().len() as isize;
        if n > 0 {
            self.step = (self.step as isize + by).rem_euclid(n) as usize;
        }
    }
}

/// How the editor shows a tab: its text, its graph beside it, or its graph alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Show {
    #[default]
    Code,
    Both,
    Graph,
}

impl Show {
    /// What ctrl+b goes to next.
    pub(crate) fn next(self) -> Show {
        match self {
            Show::Code => Show::Both,
            Show::Both => Show::Graph,
            Show::Graph => Show::Code,
        }
    }
}

/// An open flow's text.
pub(crate) struct Tab {
    pub(crate) name: String,
    /// The file it was opened from, which saving writes.
    pub(crate) path: Option<PathBuf>,
    /// The deployment it deploys as, once known.
    pub(crate) slug: Option<String>,
    pub(crate) buf: Buffer,
    /// The text as last saved or opened, to tell an edited tab.
    pub(crate) kept: String,
    pub(crate) check: Check,
    /// Edited since checked, and when.
    pub(crate) edited: Option<Instant>,
    /// What its deployment may do now, to mark what a deploy would add.
    pub(crate) deployed: Option<Vec<String>>,
    /// "View runs", when on.
    pub(crate) runs: Option<Runs>,
    /// Started here (ctrl+n), not saved yet: its first save asks for a file.
    pub(crate) draft: bool,
    /// "Graph", when on: the text as statements and wires.
    pub(crate) graph: Option<super::graph::TabGraph>,
}

impl Tab {
    pub(crate) fn new(name: &str, text: &str, path: Option<PathBuf>, slug: Option<String>) -> Tab {
        let buf = Buffer::new(text);
        let kept = buf.text();
        Tab { name: name.to_string(), path, slug, check: Check::of(&kept), kept, buf, edited: None, deployed: None, runs: None, graph: None, draft: false }
    }

    pub(crate) fn dirty(&self) -> bool {
        self.buf.text() != self.kept
    }

    /// The deployment it deploys as: its own, else the flow's name.
    pub(crate) fn target(&self) -> Option<String> {
        self.slug.clone().or_else(|| self.check.plan.as_ref().ok().map(|p| p.name.clone()))
    }
}

/// What may come at the cursor, as the text's completions offer it.
pub(crate) struct Complete {
    pub(crate) items: Vec<editing::Completion>,
    pub(crate) sel: usize,
    /// How many characters before the cursor taking one replaces.
    pub(crate) back: usize,
}

/// A line a dialog shows, and how it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Plain,
    Dim,
    Head,
    Good,
    Warn,
    Bad,
    Added,
    Removed,
}

pub(crate) type Said = (Tone, String);

pub(crate) enum Modal {
    /// A deploy, staged: what it asks for, for a person to approve by deploying it, and what it
    /// changes in the text deployed now.
    Approve { slug: String, plan: Box<agent::Plan>, staged: Staged, text: String, tab: usize, diff: Vec<super::diff::Line>, show_diff: bool, scroll: usize },
    /// Lines to read: a run explained, a cell's history, an answer.
    Lines { title: String, lines: Vec<Said>, scroll: usize },
    Confirm { title: String, lines: Vec<String>, then: Then },
    Help { scroll: usize },
    /// Everything there is to do and open, by name.
    Palette { query: String, sel: usize },
    /// Going back to a point: its code, its state, or both.
    Restore { slug: String, point: Point, sel: usize },
    /// What a restore would do, as its dry run answers: confirming it restores.
    DryRun { slug: String, point: Point, code: bool, state: bool, lines: Vec<Said>, needs: Vec<String>, refused: bool, scroll: usize },
    /// Sending a deployment a request, running an action, firing a clock.
    Call(CallForm),
    /// A line typed in: a secret's value (shown as dots), a session's number.
    Input { title: String, lines: Vec<String>, value: String, masked: bool, then: Ask },
}

pub(crate) enum Then {
    Lifecycle(String, &'static str, Option<Value>),
    Quit,
    Close(usize),
    Remove(String),
    Grant { slug: String, item: String, back: bool },
    RemoveSecret(String, String),
    Admin(super::admin::AdminDo),
}

pub(crate) enum Ask {
    Secret(String, String),
    Admin(super::admin::AdminAsk),
    /// A file to save tab `0` to (a draft's first save).
    SaveAs(usize),
}

/// The line at the bottom: what just happened.
pub(crate) struct Note {
    pub(crate) text: String,
    pub(crate) error: bool,
}

/// A list something on screen can pick from, by click or wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    Page(Page),
    Section,
    Detail,
    Complete,
    Palette,
    Restore,
    Call,
    /// The editor's problems: a click goes to one.
    Problems,
    /// A tab's runs, in "View runs", and the picked run's timeline: its steps.
    Runs,
    Timeline,
    /// A tab's graph, its rows and the picked one's detail beside them.
    Graph,
    GraphPane,
    /// The admin page's list, and the picked row beside it.
    Admin,
    AdminPane,
    /// A dialog's lines.
    Dialog,
}

/// What a place on screen does when clicked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Click {
    Page(Page),
    /// A button: as if its key were pressed.
    Key(KeyCode, KeyModifiers),
    Row(List, usize),
    Section(super::flow::Section),
    /// One of the admin page's sections.
    Part(super::admin::Part),
    DetailTab(usize),
    Tab(usize),
    CloseTab(usize),
    /// The text pane: its first character's place on screen, and the first line and column shown.
    Text { x: u16, y: u16, top: usize, left: usize },
    /// Somewhere the wheel scrolls.
    Scroll(List),
    /// Code, graph, or both.
    Show(Show),
    /// Behind a dialog: a click there closes it, as esc does.
    Outside,
}

pub(crate) struct App {
    /// `--server`, to connect again.
    pub(crate) server_arg: Option<String>,
    pub(crate) client: Option<Client>,
    /// Why the server can't be reached, when it can't.
    pub(crate) offline: Option<String>,
    pub(crate) page: Page,
    /// Each list page's selection, by page.
    pub(crate) sel: [usize; 7],
    pub(crate) detail: Option<Detail>,
    pub(crate) flow: FlowView,
    pub(crate) admin: super::admin::AdminView,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) tab: usize,
    /// Code, graph, or both: for every tab.
    pub(crate) show: Show,
    /// The editor has the keyboard: keys type.
    pub(crate) typing: bool,
    pub(crate) complete: Option<Complete>,
    /// Find, while open: what is looked for.
    pub(crate) find: Option<String>,
    /// Find was opened on a selection, which is its query until something is typed.
    find_seeded: bool,
    /// What copy and cut took, for paste.
    pub(crate) clipboard: String,
    /// Copied, for the terminal's own clipboard (sent as it draws next).
    pub(crate) to_clipboard: Option<String>,
    pub(crate) modal: Option<Modal>,
    /// The `:` prompt's text, while open.
    pub(crate) prompt: Option<String>,
    /// The shell line's text, while open (shell.rs).
    pub(crate) shell: Option<String>,
    pub(crate) shell_history: Vec<String>,
    /// Where ↑ is in the history, while going back through it.
    pub(crate) shell_back: Option<usize>,
    /// A command for the event loop to run, with the terminal handed to it.
    pub(crate) run: Option<String>,
    pub(crate) note: Option<Note>,
    pub(crate) quit: bool,
    pub(crate) refreshed: Instant,
    /// The text pane's height when last drawn, for paging.
    pub(crate) text_height: usize,
    /// Where each clickable thing was drawn last, the last drawn on top.
    pub(crate) hits: RefCell<Vec<(Rect, Click)>>,
    /// Where the mouse went down, while it is held: a drag in the text selects.
    pressed: Option<(Rect, Click)>,
    last_click: Option<Click>,
}

/// How often the pages are read again.
const REFRESH: Duration = Duration::from_secs(5);
/// How long after the last key a tab is checked again.
const SETTLE: Duration = Duration::from_millis(200);

fn index(page: Page) -> usize {
    PAGES.iter().position(|(p, _)| *p == page).unwrap_or(0)
}

impl App {
    pub(crate) fn new(server_arg: Option<String>) -> App {
        App {
            server_arg,
            client: None,
            offline: None,
            page: Page::Overview,
            sel: [0; 7],
            detail: None,
            flow: FlowView::default(),
            admin: Default::default(),
            tabs: vec![],
            tab: 0,
            show: Show::Code,
            typing: false,
            complete: None,
            find: None,
            find_seeded: false,
            clipboard: String::new(),
            to_clipboard: None,
            modal: None,
            prompt: None,
            shell: None,
            shell_history: vec![],
            shell_back: None,
            run: None,
            note: None,
            quit: false,
            refreshed: Instant::now(),
            text_height: 20,
            hits: RefCell::new(vec![]),
            pressed: None,
            last_click: None,
        }
    }

    /// Find the server (as `holon deploy` does) and read it.
    pub(crate) fn connect(&mut self) {
        match crate::local::target(self.server_arg.as_deref()) {
            Ok(t) => self.attach(Client::new(Box::new(crate::deploycmd::Person { server: t.server, token: t.token }))),
            Err(e) => {
                self.client = None;
                self.offline = Some(e);
            }
        }
    }

    pub(crate) fn attach(&mut self, client: Client) {
        self.client = Some(client);
        self.refresh();
    }

    pub(crate) fn say(&mut self, text: impl Into<String>) {
        self.note = Some(Note { text: text.into(), error: false });
    }

    pub(crate) fn fail(&mut self, text: impl Into<String>) {
        self.note = Some(Note { text: text.into(), error: true });
    }

    /// Read the server again: its lists, and the open flow's pages.
    pub(crate) fn refresh(&mut self) {
        self.refreshed = Instant::now();
        let Some(client) = self.client.as_mut() else {
            return;
        };
        match client.refresh() {
            Ok(()) => self.offline = None,
            Err(e) => {
                self.offline = Some(e);
                return;
            }
        }
        if let Some(slug) = self.detail.as_ref().map(|d| d.slug.clone()) {
            match self.client.as_ref().map(|c| c.detail(&slug)) {
                Some(Ok(d)) => {
                    self.detail = Some(d);
                    self.flow_refreshed();
                }
                Some(Err(_)) => {
                    self.detail = None;
                    if self.page == Page::Flow {
                        self.page = Page::Flows;
                    }
                }
                None => {}
            }
        }
        if self.page == Page::Admin {
            self.admin_load();
        }
    }

    /// What the clock does between keys: read the server again now and then, and check an
    /// edited tab once typing settles.
    pub(crate) fn tick(&mut self) {
        if self.refreshed.elapsed() >= REFRESH && self.modal.is_none() && self.prompt.is_none() && self.shell.is_none() {
            if self.client.is_some() {
                self.refresh();
            } else {
                self.refreshed = Instant::now();
            }
        }
        for tab in &mut self.tabs {
            if tab.edited.is_some_and(|at| at.elapsed() >= SETTLE) {
                tab.check = Check::of(&tab.buf.text());
                tab.edited = None;
            }
        }
    }

    // ── opening ─────────────────────────────────────────────────────────────────────────────

    /// What `holon editor <x>` opens: a file if there is one by that name, else a deployment's
    /// page, else a saved flow.
    pub(crate) fn open_arg(&mut self, arg: &str) {
        let path = PathBuf::from(arg);
        if path.exists() || arg.ends_with(".holon") {
            return self.open_file(path);
        }
        if self.client.as_ref().is_some_and(|c| c.deployment(arg).is_some()) {
            return self.open_flow(arg);
        }
        if self.client.as_ref().is_some_and(|c| c.saved(arg).is_some()) {
            return self.edit(arg);
        }
        self.fail(format!("no file, deployment or saved flow named \"{arg}\""));
    }

    pub(crate) fn open_file(&mut self, path: PathBuf) {
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_ref() == Some(&path)) {
            return self.show_tab(i);
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            // A new file: start it, and saving writes it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("flow").to_string();
                format!("flow {name}\n")
            }
            Err(e) => return self.fail(format!("{}: {e}", path.display())),
        };
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("flow").to_string();
        let mut tab = Tab::new(&name, &text, Some(path), None);
        tab.kept = std::fs::read_to_string(tab.path.as_ref().unwrap()).map(|t| Buffer::new(&t).text()).unwrap_or_default();
        self.add_tab(tab);
    }

    fn add_tab(&mut self, mut tab: Tab) {
        if let (Some(c), Some(slug)) = (self.client.as_ref(), tab.target()) {
            tab.deployed = c.deployment(&slug).and_then(|_| c.get(&format!("/deployments/{}/manifest", crate::mcp::seg(&slug))).ok()).map(|m| client::strings(&m, "items"));
            if tab.deployed.is_some() && tab.slug.is_none() {
                tab.slug = Some(slug);
            }
        }
        self.tabs.push(tab);
        self.show_tab(self.tabs.len() - 1);
        self.typing = true;
    }

    pub(crate) fn show_tab(&mut self, i: usize) {
        self.tab = i;
        self.page = Page::Editor;
        self.complete = None;
        self.find = None;
    }

    /// Open a deployment's or saved flow's text in a tab: the saved one when it is newer than
    /// what is deployed (someone saved it for a person to review), else the deployed one.
    pub(crate) fn edit(&mut self, name: &str) {
        if let Some(i) = self.tabs.iter().position(|t| t.path.is_none() && t.slug.as_deref() == Some(name)) {
            self.typing = true;
            return self.show_tab(i);
        }
        let Some(c) = self.client.as_ref() else {
            return self.fail("no server to open it from");
        };
        let deployed_at = c.deployment(name).and_then(|d| d["deployed_at_epoch_seconds"].as_u64());
        let saved_at = c.saved(name).and_then(|f| f["saved_at_epoch_seconds"].as_u64());
        let text = match (deployed_at, saved_at) {
            (Some(d), Some(s)) if s > d => c.saved_text(name),
            (Some(_), _) => c.text(name, None),
            (None, Some(_)) => c.saved_text(name),
            (None, None) => Err(format!("nothing named \"{name}\" is deployed or saved")),
        };
        match text {
            Ok(t) => {
                let tab = Tab::new(name, &t, None, Some(name.to_string()));
                self.add_tab(tab);
            }
            Err(e) => self.fail(e),
        }
    }

    /// A new draft, as `holon new` starts one, in a tab of its own: its first save asks for a file.
    pub(crate) fn new_draft(&mut self) {
        let text = holon_author::commands::new_flow(None).unwrap_or_default();
        let name = (1..).map(|n| format!("draft {n}")).find(|n| !self.tabs.iter().any(|t| t.name == *n)).unwrap_or_default();
        let mut tab = Tab::new(&name, &text, None, None);
        tab.draft = true;
        tab.kept = String::new();
        self.add_tab(tab);
        self.say("a new draft: ctrl+s asks where to save it");
    }

    /// A draft's first save: to the file named, from the folder the editor is in.
    pub(crate) fn save_as(&mut self, i: usize, file: &str) {
        let file = file.trim();
        if file.is_empty() {
            return self.fail("not saved: no file named");
        }
        let home = || std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
        let path = match file.strip_prefix("~/").or_else(|| file.strip_prefix("~\\")) {
            Some(rest) => home().join(rest),
            None => PathBuf::from(file),
        };
        if path.exists() {
            return self.fail(format!("{} is there already: another name, or :w {file} to write over it", path.display()));
        }
        self.save(i, Some(path));
    }

    pub(crate) fn open_sample(&mut self, name: &str) {
        match samples::sample(name) {
            Some(s) => {
                let mut tab = Tab::new(&format!("{} (sample)", s.name), s.text, None, None);
                // Unsaved until saved: a copy, as the browser's editor opens one.
                tab.kept = String::new();
                self.add_tab(tab);
            }
            None => self.fail(format!("no sample \"{name}\" (the library lists them)")),
        }
    }

    pub(crate) fn open_flow(&mut self, slug: &str) {
        let Some(c) = self.client.as_ref() else {
            return self.fail("no server");
        };
        match c.detail(slug) {
            Ok(d) => {
                if self.detail.as_ref().map(|o| &o.slug) != Some(&d.slug) {
                    self.flow = FlowView::default();
                }
                self.detail = Some(d);
                self.page = Page::Flow;
                self.flow_refreshed();
            }
            Err(e) => self.fail(e),
        }
    }

    // ── what the pages list ─────────────────────────────────────────────────────────────────

    /// The flows page's rows: every deployment and saved flow, by name.
    pub(crate) fn flow_names(&self) -> Vec<String> {
        let Some(c) = self.client.as_ref() else { return vec![] };
        let mut names: Vec<String> = c.deployments.iter().chain(&c.flows).filter_map(|v| v.get("slug").or(v.get("name")).and_then(Value::as_str).map(str::to_string)).collect();
        names.sort();
        names.dedup();
        names
    }

    pub(crate) fn waiting(&self) -> Vec<(String, String)> {
        self.client.as_ref().map(Client::waiting).unwrap_or_default()
    }

    fn rows(&self, page: Page) -> usize {
        match page {
            Page::Overview => self.client.as_ref().map_or(0, |c| c.deployments.len()),
            Page::Approvals => self.waiting().len(),
            Page::Flows => self.flow_names().len(),
            Page::Library => samples::samples().len(),
            Page::Flow | Page::Editor | Page::Admin => 0,
        }
    }

    // ── doing ──────────────────────────────────────────────────────────────────────────────

    /// Save a tab: to its file, else on the server as a saved flow.
    pub(crate) fn save(&mut self, i: usize, to: Option<PathBuf>) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        // A draft has no file yet: ask for one, named as its flow.
        if to.is_none() && tab.path.is_none() && tab.draft {
            let name = agent::plan(&tab.buf.text()).map(|p| p.name).unwrap_or_else(|_| "flow".into());
            let here = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
            self.modal = Some(Modal::Input {
                title: format!("Save {} as", tab.name),
                lines: vec![format!("A file, from {here} (~ is your home folder):"), "Enter writes it there; esc leaves it a draft.".into()],
                value: format!("{name}.holon"),
                masked: false,
                then: Ask::SaveAs(i),
            });
            return;
        }
        if let Some(p) = to {
            tab.name = p.file_name().and_then(|s| s.to_str()).unwrap_or("flow").to_string();
            tab.path = Some(p);
            tab.draft = false;
        }
        let text = tab.buf.text();
        if let Some(path) = tab.path.clone() {
            match std::fs::write(&path, &text) {
                Ok(()) => {
                    tab.kept = text;
                    self.say(format!("wrote {}", path.display()));
                }
                Err(e) => self.fail(format!("{}: {e}", path.display())),
            }
            return;
        }
        let plan = match agent::plan(&text) {
            Ok(p) => p,
            Err(e) => return self.fail(format!("not saved: {e} (:w <file> writes it to a file as it is)")),
        };
        let name = tab.slug.clone().unwrap_or_else(|| plan.name.clone());
        let Some(c) = self.client.as_ref() else {
            return self.fail("no server to save it on: :w <file> writes it to a file");
        };
        match c.save(&name, &plan, &text) {
            Ok(()) => {
                let tab = &mut self.tabs[i];
                tab.kept = text;
                tab.slug = Some(name.clone());
                tab.name = name.clone();
                self.refresh();
                self.say(format!("saved \"{name}\" on {}, not deployed", self.client.as_ref().map(Client::server).unwrap_or_default()));
            }
            Err(e) => self.fail(e),
        }
    }

    /// Stage a tab's deploy, and show what it asks a person to approve and what it changes.
    pub(crate) fn deploy(&mut self, i: usize) {
        let Some(tab) = self.tabs.get(i) else { return };
        let text = tab.buf.text();
        let plan = match agent::plan(&text) {
            Ok(p) => p,
            Err(e) => return self.fail(format!("can't deploy: {e}")),
        };
        let slug = tab.slug.clone().unwrap_or_else(|| plan.name.clone());
        let Some(c) = self.client.as_ref() else {
            return self.fail(format!("no server to deploy to: {}", self.offline.clone().unwrap_or_else(|| "start holon serve".into())));
        };
        let diff = match c.deployment(&slug).and_then(|_| c.text(&slug, None).ok()) {
            Some(now) => super::diff::lines(&now, &text),
            None => vec![],
        };
        match c.stage(&plan, &slug) {
            Ok(staged) => self.modal = Some(Modal::Approve { slug, plan: Box::new(plan), staged, text, tab: i, diff, show_diff: false, scroll: 0 }),
            Err(e) => self.fail(e),
        }
    }

    /// Deploy what the dialog shows, approving it as us.
    fn approve(&mut self) {
        let Some(Modal::Approve { slug, plan, staged, text, tab, .. }) = self.modal.take() else { return };
        let Some(c) = self.client.as_ref() else { return };
        let needs = staged.needs.len();
        match c.commit(&plan, staged, &text) {
            Ok(()) => {
                if let Some(t) = self.tabs.get_mut(tab) {
                    t.slug = Some(slug.clone());
                    t.deployed = Some(plan.may.clone());
                    if t.path.is_none() {
                        t.kept = text;
                        t.name = slug.clone();
                    }
                }
                self.refresh();
                let approved = if needs == 0 { String::new() } else { format!(", approving {needs} new permission{} as you", if needs == 1 { "" } else { "s" }) };
                let reach = if plan.routes.is_empty() { plan.interval_seconds.map(|i| format!("; runs every {i} s")).unwrap_or_default() } else { format!("; {}", plan.routes.join(", ")) };
                self.say(format!("deployed \"{slug}\" ({}){approved}{reach}", &plan.unit[..12]));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(crate) fn lifecycle(&mut self, slug: &str, action: &str, from: Option<Value>) {
        let Some(c) = self.client.as_ref() else { return };
        match c.lifecycle(slug, action, from) {
            Ok(()) => {
                self.refresh();
                let done = match action {
                    "run" => "running",
                    "pause" => "paused",
                    _ => "stopped",
                };
                self.say(format!("\"{slug}\" is {done}"));
            }
            Err(e) => self.fail(e),
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            self.tab = self.tab.min(self.tabs.len().saturating_sub(1));
            if self.tabs.is_empty() {
                self.typing = false;
            }
        }
    }

    pub(crate) fn ask_close(&mut self, i: usize) {
        match self.tabs.get(i) {
            Some(t) if t.dirty() => {
                self.modal = Some(Modal::Confirm { title: format!("Close {}?", t.name), lines: vec!["It has changes that aren't saved: closing it drops them.".into()], then: Then::Close(i) })
            }
            Some(_) => self.close_tab(i),
            None => {}
        }
    }

    pub(crate) fn ask_quit(&mut self) {
        let edited: Vec<String> = self.tabs.iter().filter(|t| t.dirty()).map(|t| t.name.clone()).collect();
        if edited.is_empty() {
            self.quit = true;
        } else {
            self.modal = Some(Modal::Confirm { title: "Quit?".into(), lines: vec![format!("Not saved: {}.", edited.join(", ")), "Quitting drops those changes.".into()], then: Then::Quit });
        }
    }

    pub(crate) fn format(&mut self, i: usize) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        match editing::format(&tab.buf.text()) {
            Ok(t) => {
                tab.buf.replace(&t);
                tab.edited = Some(Instant::now() - SETTLE);
                self.say("formatted, as holon fmt writes it");
            }
            Err(e) => self.fail(format!("can't format it: {e}")),
        }
    }

    /// Turn a tab's "View runs" on or off.
    pub(crate) fn toggle_runs(&mut self, i: usize) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        if tab.runs.take().is_some() {
            self.typing = true;
            return;
        }
        let Some(slug) = tab.slug.clone().filter(|s| self.client.as_ref().is_some_and(|c| c.deployment(s).is_some())) else {
            return self.fail("it isn't deployed: deploy it (ctrl+d) to see its runs");
        };
        let c = self.client.as_ref().unwrap();
        match c.get(&format!("/deployments/{}/log?limit={}", crate::mcp::seg(&slug), client::RUNS)) {
            Ok(log) => {
                let mut entries = log["entries"].as_array().cloned().unwrap_or_default();
                entries.reverse();
                let mut runs = Runs { slug, entries, sel: 0, view: None, step: 0 };
                load_run(c, &mut runs);
                self.tabs[i].runs = Some(runs);
                self.typing = false;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Open a run in the editor, step by step over its version's text.
    pub(crate) fn step_through(&mut self, slug: &str, seq: u64) {
        self.edit(slug);
        if self.page != Page::Editor {
            return;
        }
        let i = self.tab;
        if self.tabs[i].runs.is_none() {
            self.toggle_runs(i);
        }
        let Some(runs) = self.tabs[i].runs.as_mut() else { return };
        if let Some(at) = runs.entries.iter().position(|e| e["seq"].as_u64() == Some(seq)) {
            runs.sel = at;
            self.reload_run(i);
        }
        self.typing = false;
    }

    pub(crate) fn run_command(&mut self, line: &str) {
        let line = line.trim();
        // `:!command`, as vim has it: the shell line's.
        if let Some(command) = line.strip_prefix('!') {
            return match command.trim() {
                "" => self.open_shell(),
                c => self.shell_command(c),
            };
        }
        let (cmd, arg) = line.split_once(' ').map(|(c, a)| (c, a.trim())).unwrap_or((line, ""));
        let tab = (!self.tabs.is_empty()).then_some(self.tab);
        if let Ok(n) = cmd.parse::<u32>() {
            return match tab {
                Some(i) => {
                    self.tabs[i].buf.go(n, 1);
                    self.page = Page::Editor;
                    self.typing = true;
                }
                None => self.fail("no text to go to a line of"),
            };
        }
        match (cmd, arg) {
            ("q" | "quit", _) => self.ask_quit(),
            ("q!", _) => self.quit = true,
            ("e" | "open" | "edit", a) if !a.is_empty() => self.open_arg(a),
            ("new", "") => self.new_draft(),
            ("new", a) => self.open_sample(a),
            ("w" | "save", a) => match tab {
                Some(i) => self.save(i, (!a.is_empty()).then(|| PathBuf::from(a))),
                None => self.fail("no tab to save"),
            },
            ("deploy", _) => match tab {
                Some(i) => self.deploy(i),
                None => self.fail("no tab to deploy: open a flow first"),
            },
            ("fmt" | "format", _) => match tab {
                Some(i) => self.format(i),
                None => self.fail("no tab to format"),
            },
            ("close", _) => match tab {
                Some(i) => self.ask_close(i),
                None => self.fail("no tab to close"),
            },
            ("flow", a) if !a.is_empty() => self.open_flow(a),
            (action @ ("run" | "pause" | "stop"), a) if !a.is_empty() => self.lifecycle(a, action, None),
            ("call", a) => {
                let slug = if a.is_empty() { self.detail.as_ref().map(|d| d.slug.clone()) } else { Some(a.to_string()) };
                match slug {
                    Some(s) => self.call(&s),
                    None => self.fail("call which flow? :call <slug>"),
                }
            }
            ("restore", a) => {
                let slug = if a.is_empty() { self.detail.as_ref().map(|d| d.slug.clone()) } else { Some(a.to_string()) };
                match slug {
                    Some(s) => {
                        self.open_flow(&s);
                        self.flow.section = super::flow::Section::History;
                    }
                    None => self.fail("restore which flow? :restore <slug>"),
                }
            }
            ("connect" | "server", a) => {
                if !a.is_empty() {
                    self.server_arg = Some(a.to_string());
                }
                self.connect();
                if self.offline.is_none() {
                    self.say(format!("connected to {}", self.client.as_ref().map(Client::server).unwrap_or_default()));
                }
            }
            ("sessions", a) => {
                let slug = if a.is_empty() { self.detail.as_ref().map(|d| d.slug.clone()) } else { Some(a.to_string()) };
                let Some(slug) = slug else { return self.fail("whose sessions? :sessions <slug>") };
                match self.client.as_ref().map(|c| c.get(&format!("/deployments/{}/sessions", crate::mcp::seg(&slug)))) {
                    Some(Ok(v)) => {
                        let lines = crate::opscmd::session_lines(&slug, &v).into_iter().map(|l| (if l.starts_with("session") { Tone::Head } else { Tone::Plain }, l)).collect();
                        self.modal = Some(Modal::Lines { title: format!("Sessions of {slug}"), lines, scroll: 0 });
                    }
                    Some(Err(e)) => self.fail(e),
                    None => self.fail("no server"),
                }
            }
            ("r" | "refresh", _) => self.refresh(),
            ("help" | "h", _) => self.modal = Some(Modal::Help { scroll: 0 }),
            _ => self.fail(format!(":{line}? (:help lists the commands; ctrl+p finds anything)")),
        }
    }

    // ── keys ───────────────────────────────────────────────────────────────────────────────

    pub(crate) fn paste(&mut self, text: &str) {
        if let Some(p) = self.prompt.as_mut().or(self.shell.as_mut()) {
            p.push_str(text.lines().next().unwrap_or(""));
        } else if let Some(Modal::Input { value, .. } | Modal::Palette { query: value, .. }) = self.modal.as_mut() {
            value.push_str(text.lines().next().unwrap_or(""));
        } else if let Some(Modal::Call(form)) = self.modal.as_mut() {
            form.paste(text);
        } else if self.page == Page::Editor && self.typing && let Some(tab) = self.tabs.get_mut(self.tab) {
            tab.buf.insert_str(text);
            tab.edited = Some(Instant::now());
        }
    }

    pub(crate) fn key(&mut self, k: KeyEvent) {
        // AltGr arrives as ctrl+alt: a character typed, not a shortcut.
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let in_text = self.page == Page::Editor && self.typing && self.modal.is_none() && self.prompt.is_none();
        if ctrl && k.code == KeyCode::Char('c') {
            if in_text && self.copy(false) {
                return;
            }
            return self.ask_quit();
        }
        // What just happened stays until the next key.
        self.note = None;
        if self.modal.is_some() {
            return self.modal_key(k);
        }
        if self.prompt.is_some() {
            return self.prompt_key(k);
        }
        if self.shell.is_some() {
            return self.shell_key(k);
        }
        // ctrl+` opens the shell line from anywhere, typing included (! does where keys aren't text).
        if ctrl && k.code == KeyCode::Char('`') {
            return self.open_shell();
        }
        if ctrl && k.code == KeyCode::Char('n') {
            return self.new_draft();
        }
        if ctrl && matches!(k.code, KeyCode::Char('p') | KeyCode::Char('k')) {
            self.modal = Some(Modal::Palette { query: String::new(), sel: 0 });
            return;
        }
        if self.page == Page::Editor && !self.tabs.is_empty() {
            if ctrl && self.editor_shortcut(k) {
                return;
            }
            if k.modifiers.contains(KeyModifiers::ALT) && !k.modifiers.contains(KeyModifiers::CONTROL) {
                match k.code {
                    KeyCode::Char('f' | 'F') => return self.format(self.tab),
                    KeyCode::Left => return self.show_tab((self.tab + self.tabs.len() - 1) % self.tabs.len()),
                    KeyCode::Right => return self.show_tab((self.tab + 1) % self.tabs.len()),
                    _ => {}
                }
            }
            if self.typing && self.tabs[self.tab].runs.is_none() && self.show != Show::Graph {
                return self.type_key(k, ctrl);
            }
        }
        self.page_key(k);
    }

    /// The editor's ctrl keys, typing or not.
    fn editor_shortcut(&mut self, k: KeyEvent) -> bool {
        let i = self.tab;
        let typing = self.typing && self.tabs[i].runs.is_none() && self.show != Show::Graph;
        match k.code {
            KeyCode::Char('s') => self.save(i, None),
            KeyCode::Char('d') => self.deploy(i),
            KeyCode::Char('r') => self.toggle_runs(i),
            KeyCode::Char('b') => self.next_show(),
            KeyCode::Char('w') => self.ask_close(i),
            KeyCode::PageDown => self.show_tab((i + 1) % self.tabs.len()),
            KeyCode::PageUp => self.show_tab((i + self.tabs.len() - 1) % self.tabs.len()),
            KeyCode::Char('q') => self.ask_quit(),
            KeyCode::Char('g') => self.prompt = Some(String::new()),
            KeyCode::Char('f') if typing => {
                self.complete = None;
                let seed = self.tabs[i].buf.selected().filter(|s| !s.contains('\n')).unwrap_or_default();
                self.find_seeded = !seed.is_empty();
                self.find = Some(seed);
            }
            KeyCode::Char('z') if typing => self.edit_buf(|b| b.undo(), "nothing to undo"),
            KeyCode::Char('y') if typing => self.edit_buf(|b| b.redo(), "nothing to redo"),
            KeyCode::Char('a') if typing => self.tabs[i].buf.select_all(),
            KeyCode::Char('x') if typing => {
                self.copy(true);
            }
            KeyCode::Char('v') if typing => {
                let text = self.clipboard.clone();
                self.paste(&text);
            }
            KeyCode::Char('/' | '7' | '#') if typing => self.edit_buf(|b| { b.toggle_comment(); true }, ""),
            KeyCode::Char(' ') if typing => self.offer(true),
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Backspace if typing => return false,
            _ => return false,
        }
        true
    }

    /// An edit the buffer makes itself; `none` says when there was nothing to do.
    fn edit_buf(&mut self, f: impl FnOnce(&mut Buffer) -> bool, none: &str) {
        let tab = &mut self.tabs[self.tab];
        if f(&mut tab.buf) {
            tab.edited = Some(Instant::now());
        } else if !none.is_empty() {
            self.fail(none);
        }
    }

    fn type_key(&mut self, k: KeyEvent, ctrl: bool) {
        if self.find.is_some() {
            return self.find_key(k);
        }
        if self.complete.is_some() && self.complete_key(k.code) {
            return;
        }
        let height = self.text_height;
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let alt = k.modifiers.contains(KeyModifiers::ALT) && !k.modifiers.contains(KeyModifiers::CONTROL);
        let tab = &mut self.tabs[self.tab];
        let b = &mut tab.buf;
        let before = b.lines.clone();
        let mut typed = None;
        match k.code {
            KeyCode::Esc if b.anchor.is_some() => b.anchor = None,
            KeyCode::Esc => self.typing = false,
            KeyCode::Up | KeyCode::Down if alt && shift => b.duplicate_line(),
            KeyCode::Up if alt => b.move_lines(false),
            KeyCode::Down if alt => b.move_lines(true),
            KeyCode::Char(c) if !ctrl && !alt => {
                b.insert(c);
                typed = Some(c);
            }
            KeyCode::Enter => b.newline(true),
            KeyCode::Tab => b.tab(),
            KeyCode::BackTab => b.outdent(),
            KeyCode::Backspace if ctrl || alt => {
                b.mark(true);
                b.word_left();
                b.delete_selection();
            }
            KeyCode::Backspace => b.backspace(),
            KeyCode::Delete => b.delete(),
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown => {
                b.mark(shift);
                match k.code {
                    KeyCode::Left if ctrl => b.word_left(),
                    KeyCode::Right if ctrl => b.word_right(),
                    KeyCode::Left => b.left(),
                    KeyCode::Right => b.right(),
                    KeyCode::Up => b.down(-1),
                    KeyCode::Down => b.down(1),
                    KeyCode::Home if ctrl => (b.row, b.col) = (0, 0),
                    KeyCode::End if ctrl => (b.row, b.col) = (b.lines.len() - 1, usize::MAX),
                    KeyCode::Home => b.home(),
                    KeyCode::End => b.end(),
                    KeyCode::PageUp => b.down(-(height as isize)),
                    _ => b.down(height as isize),
                }
                b.col = b.col.min(b.lines[b.row].chars().count());
            }
            _ => {}
        }
        if b.lines != before {
            tab.edited = Some(Instant::now());
        }
        // A name being typed, or a `.` after one, offers what may come next.
        match typed {
            Some(c) if c == '.' || ((c.is_alphanumeric() || c == '_') && tab.buf.word_before().1.chars().count() >= 2) => self.offer(false),
            Some(_) => self.complete = None,
            None if k.code == KeyCode::Backspace && self.complete.is_some() => self.offer(false),
            None => self.complete = None,
        }
    }

    /// What may come at the cursor; `asked` (ctrl+space) shows it even before a letter.
    pub(crate) fn offer(&mut self, asked: bool) {
        let Some(tab) = self.tabs.get(self.tab) else { return };
        let (start, word) = tab.buf.word_before();
        let after_dot = start > 0 && tab.buf.lines[tab.buf.row].chars().nth(start - 1) == Some('.');
        if word.is_empty() && !asked && !after_dot {
            self.complete = None;
            return;
        }
        let low = word.to_lowercase();
        let items: Vec<editing::Completion> =
            editing::complete(&tab.buf.text(), cursor(tab)).into_iter().filter(|c| c.label.to_lowercase().starts_with(&low) && c.label != word).take(40).collect();
        self.complete = (!items.is_empty()).then(|| Complete { items, sel: 0, back: tab.buf.col.min(tab.buf.lines[tab.buf.row].chars().count()) - start });
    }

    /// The completion list's keys; false lets the key through to the text.
    fn complete_key(&mut self, code: KeyCode) -> bool {
        let Some(c) = self.complete.as_mut() else { return false };
        match code {
            KeyCode::Up => c.sel = (c.sel + c.items.len() - 1) % c.items.len(),
            KeyCode::Down => c.sel = (c.sel + 1) % c.items.len(),
            KeyCode::Esc => self.complete = None,
            KeyCode::Tab | KeyCode::Enter => self.take_completion(None),
            _ => return false,
        }
        true
    }

    pub(crate) fn take_completion(&mut self, i: Option<usize>) {
        let Some(c) = self.complete.take() else { return };
        let Some(item) = c.items.get(i.unwrap_or(c.sel)) else { return };
        let tab = &mut self.tabs[self.tab];
        tab.buf.complete(c.back, &item.label);
        tab.edited = Some(Instant::now());
    }

    /// Find's keys: typing searches as it goes, enter goes to the next, shift+enter back.
    fn find_key(&mut self, k: KeyEvent) {
        let Some(q) = self.find.as_mut() else { return };
        let b = &mut self.tabs[self.tab].buf;
        let found = match k.code {
            KeyCode::Esc => {
                self.find = None;
                return;
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::F(3) => b.find(q, k.modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::Up => b.find(q, true),
            KeyCode::Backspace => {
                q.pop();
                b.anchor = None;
                q.is_empty() || b.find(q, false)
            }
            KeyCode::Char(c) => {
                // A query taken from the selection is replaced by what is typed first.
                if std::mem::take(&mut self.find_seeded) {
                    q.clear();
                }
                q.push(c);
                // From where this match starts, so the match grows as the word is typed.
                if let Some(((r, col), _)) = b.selection() {
                    (b.row, b.col) = (r, col);
                    b.anchor = None;
                }
                b.find(q, false)
            }
            _ => true,
        };
        if !found {
            let q = q.clone();
            self.fail(format!("\"{q}\" isn't in it"));
        }
    }

    /// Copy the selection, here and to the terminal's clipboard.
    fn copy(&mut self, cut: bool) -> bool {
        let Some(tab) = self.tabs.get_mut(self.tab) else { return false };
        let Some(text) = tab.buf.selected() else { return false };
        if cut {
            tab.buf.delete_selection();
            tab.edited = Some(Instant::now());
        }
        let n = text.lines().count().max(1);
        self.say(format!("{} {n} line{}", if cut { "cut" } else { "copied" }, if n == 1 { "" } else { "s" }));
        self.clipboard = text.clone();
        self.to_clipboard = Some(text);
        true
    }

    fn select(&mut self, page: Page, by: isize) {
        let n = self.rows(page);
        let s = &mut self.sel[index(page)];
        *s = (*s as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
    }

    fn page_key(&mut self, k: KeyEvent) {
        let page = self.page;
        match k.code {
            KeyCode::Char(c @ '1'..='7') => {
                let to = PAGES[c as usize - '1' as usize].0;
                return self.go(to);
            }
            KeyCode::Char(':') => return self.prompt = Some(String::new()),
            KeyCode::Char('!') => return self.open_shell(),
            KeyCode::Char('?') => return self.modal = Some(Modal::Help { scroll: 0 }),
            KeyCode::Char('q') => return self.ask_quit(),
            KeyCode::Char('/') if page != Page::Flow => return self.modal = Some(Modal::Palette { query: String::new(), sel: 0 }),
            KeyCode::Char('r') if page != Page::Editor => {
                if self.client.is_none() {
                    self.connect();
                } else {
                    self.refresh();
                }
                return self.say("read the server again");
            }
            _ => {}
        }
        if page == Page::Flow {
            return self.flow_key(k);
        }
        if page == Page::Admin {
            return self.admin_key(k);
        }
        if page == Page::Editor {
            if k.code == KeyCode::Char('n') && self.tabs.get(self.tab).is_none_or(|t| t.runs.is_none()) && self.show != Show::Graph {
                return self.new_draft();
            }
            return self.editor_key(k);
        }
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => return self.select(page, -1),
            KeyCode::Down | KeyCode::Char('j') => return self.select(page, 1),
            KeyCode::PageUp => return self.select(page, -10),
            KeyCode::PageDown => return self.select(page, 10),
            KeyCode::Home => return self.select(page, isize::MIN / 2),
            KeyCode::End => return self.select(page, isize::MAX / 2),
            _ => {}
        }
        let sel = self.sel[index(page)];
        match page {
            Page::Overview => {
                let slug = self.client.as_ref().and_then(|c| c.deployments.get(sel)).and_then(|d| d["slug"].as_str()).map(str::to_string);
                match (k.code, slug) {
                    (KeyCode::Enter, Some(s)) => self.open_flow(&s),
                    (KeyCode::Char('e'), Some(s)) => self.edit(&s),
                    (KeyCode::Char('c'), Some(s)) => self.call(&s),
                    _ => {}
                }
            }
            Page::Approvals => {
                if let (KeyCode::Enter, Some((slug, wait))) = (k.code, self.waiting().get(sel).cloned()) {
                    self.resolve(&slug, &wait);
                }
            }
            Page::Flows => {
                let Some(name) = self.flow_names().get(sel).cloned() else { return };
                let deployed = self.client.as_ref().is_some_and(|c| c.deployment(&name).is_some());
                match k.code {
                    KeyCode::Enter if deployed => self.open_flow(&name),
                    KeyCode::Enter | KeyCode::Char('e') => self.edit(&name),
                    KeyCode::Char('c') if deployed => self.call(&name),
                    _ => {}
                }
            }
            Page::Library => {
                if k.code == KeyCode::Enter
                    && let Some(s) = samples::samples().get(sel)
                {
                    self.open_sample(s.name);
                }
            }
            Page::Flow | Page::Editor | Page::Admin => {}
        }
    }

    pub(crate) fn go(&mut self, to: Page) {
        if to == Page::Flow && self.detail.is_none() {
            return self.fail("no flow open: pick one on the flows page");
        }
        self.page = to;
        if to == Page::Admin {
            self.admin_load();
        }
        if to == Page::Editor && !self.tabs.is_empty() && self.tabs[self.tab].runs.is_none() && self.show != Show::Graph {
            self.typing = true;
        }
    }

    /// Do what a deployment waits on a person for: start it, or review and deploy its text.
    fn resolve(&mut self, slug: &str, wait: &str) {
        if wait.starts_with("to start it") {
            self.modal = Some(Modal::Confirm { title: format!("Run {slug}?"), lines: vec![format!("\"{slug}\" is {}: run it again, as you.", self.status_of(slug))], then: Then::Lifecycle(slug.to_string(), "run", None) });
        } else if wait.starts_with("to free its port") {
            self.open_flow(slug);
            self.fail(format!("{slug}: {wait} (stop what holds it, then deploy again)"));
        } else {
            self.edit(slug);
            if self.page == Page::Editor {
                self.deploy(self.tab);
            }
        }
    }

    pub(crate) fn status_of(&self, slug: &str) -> String {
        self.client.as_ref().and_then(|c| c.deployment(slug)).and_then(|d| d["status"].as_str()).unwrap_or("?").to_string()
    }

    fn editor_key(&mut self, k: KeyEvent) {
        let Some(i) = (!self.tabs.is_empty()).then_some(self.tab) else { return };
        if let Some(runs) = self.tabs.get_mut(i).and_then(|t| t.runs.as_mut()) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('v') => {
                    self.tabs[i].runs = None;
                    self.typing = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    runs.sel = runs.sel.saturating_sub(1);
                    self.reload_run(i);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    runs.sel = (runs.sel + 1).min(runs.entries.len().saturating_sub(1));
                    self.reload_run(i);
                }
                KeyCode::Char('n') | KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => runs.go(1),
                KeyCode::Char('N') | KeyCode::Left | KeyCode::Char('h') => runs.go(-1),
                KeyCode::Home => runs.step = 0,
                KeyCode::End => runs.step = runs.steps().len().saturating_sub(1),
                KeyCode::Char('b') => self.next_show(),
                _ => {}
            }
            return;
        }
        if self.show == Show::Graph {
            return self.tab_graph_key(k);
        }
        match k.code {
            KeyCode::Enter | KeyCode::Char('i') => self.typing = true,
            KeyCode::Char('[') => self.show_tab((i + self.tabs.len() - 1) % self.tabs.len()),
            KeyCode::Char(']') => self.show_tab((i + 1) % self.tabs.len()),
            KeyCode::Char('w') => self.ask_close(i),
            KeyCode::Char('s') => self.save(i, None),
            KeyCode::Char('d') => self.deploy(i),
            KeyCode::Char('v') => self.toggle_runs(i),
            KeyCode::Char('b') => self.next_show(),
            KeyCode::Char('f') => self.format(i),
            _ => {}
        }
    }

    pub(crate) fn reload_run(&mut self, i: usize) {
        if let (Some(c), Some(runs)) = (self.client.as_ref(), self.tabs.get_mut(i).and_then(|t| t.runs.as_mut())) {
            load_run(c, runs);
        }
    }

    fn prompt_key(&mut self, k: KeyEvent) {
        let Some(p) = self.prompt.as_mut() else { return };
        match k.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let line = self.prompt.take().unwrap_or_default();
                self.run_command(&line);
            }
            KeyCode::Backspace => {
                if p.pop().is_none() {
                    self.prompt = None;
                }
            }
            KeyCode::Char(c) => p.push(c),
            _ => {}
        }
    }

    fn modal_key(&mut self, k: KeyEvent) {
        let Some(modal) = self.modal.as_mut() else { return };
        let page = |by: isize, at: &mut usize, n: usize| *at = (*at as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        match modal {
            Modal::Approve { show_diff, scroll, diff, .. } => match k.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('a') => self.approve(),
                KeyCode::Char('d') | KeyCode::Tab if !diff.is_empty() => {
                    *show_diff = !*show_diff;
                    *scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll += 10,
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.modal = None;
                    self.say("not deployed");
                }
                _ => {}
            },
            Modal::Lines { scroll, lines, .. } => match k.code {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => page(1, scroll, lines.len()),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => page(10, scroll, lines.len()),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.modal = None,
                _ => {}
            },
            Modal::Help { scroll } => match k.code {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::PageDown => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                _ => self.modal = None,
            },
            Modal::Confirm { .. } => match k.code {
                KeyCode::Enter | KeyCode::Char('y') => self.confirm(),
                KeyCode::Esc | KeyCode::Char('n') => self.modal = None,
                _ => {}
            },
            Modal::Palette { query, sel } => match k.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel += 1,
                KeyCode::PageUp => *sel = sel.saturating_sub(10),
                KeyCode::PageDown => *sel += 10,
                KeyCode::Backspace => {
                    query.pop();
                    *sel = 0;
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    *sel = 0;
                }
                KeyCode::Enter => {
                    let (q, s) = (query.clone(), *sel);
                    self.palette_pick(&q, s);
                }
                _ => {}
            },
            Modal::Restore { .. } => self.restore_key(k),
            Modal::DryRun { .. } => self.dry_run_key(k),
            Modal::Call(_) => self.call_key(k),
            Modal::Input { value, .. } => match k.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Enter => self.answer_input(),
                _ => {}
            },
        }
    }

    fn confirm(&mut self) {
        let Some(Modal::Confirm { then, .. }) = self.modal.take() else { return };
        match then {
            Then::Quit => self.quit = true,
            Then::Close(i) => self.close_tab(i),
            Then::Lifecycle(slug, action, from) => self.lifecycle(&slug, action, from),
            Then::Remove(slug) => self.remove(&slug),
            Then::Grant { slug, item, back } => self.change_grant(&slug, &item, back),
            Then::RemoveSecret(slug, name) => self.remove_secret(&slug, &name),
            Then::Admin(d) => self.admin_do(d),
        }
    }

    /// What the palette's row `sel` is, done.
    fn palette_pick(&mut self, query: &str, sel: usize) {
        let items = palette::items(self, query);
        let Some(item) = items.get(sel.min(items.len().saturating_sub(1))).cloned() else {
            return self.fail(format!("nothing matches \"{query}\""));
        };
        self.modal = None;
        palette::run(self, &item.act);
    }

    // ── the mouse ──────────────────────────────────────────────────────────────────────────

    pub(crate) fn hit(&self, rect: Rect, click: Click) {
        self.hits.borrow_mut().push((rect, click));
    }

    fn hit_at(&self, x: u16, y: u16) -> Option<(Rect, Click)> {
        self.hits.borrow().iter().rev().find(|(r, _)| r.contains(Position { x, y })).copied()
    }

    pub(crate) fn mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some((rect, click)) = self.hit_at(m.column, m.row) else { return };
                self.note = None;
                self.pressed = Some((rect, click));
                // A row clicked twice in a row opens; once, it is picked.
                let again = self.last_click.replace(click) == Some(click);
                self.click(click, m.column.saturating_sub(rect.x), m.row.saturating_sub(rect.y), m.modifiers, again);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Held down in the text and moved: the selection runs from where it went down.
                if let Some((rect, Click::Text { top, left, .. })) = self.pressed
                    && let Some(tab) = self.tabs.get_mut(self.tab)
                {
                    let b = &mut tab.buf;
                    b.mark(true);
                    let dy = m.row as isize - rect.y as isize;
                    b.row = (top as isize + dy).clamp(0, b.lines.len() as isize - 1) as usize;
                    b.col = (left + m.column.saturating_sub(rect.x) as usize).min(b.lines[b.row].chars().count());
                    if m.row < rect.y {
                        b.top = b.row;
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.pressed = None,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let by: isize = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                let list = match self.hit_at(m.column, m.row).map(|(_, c)| c) {
                    Some(Click::Row(l, _) | Click::Scroll(l)) => Some(l),
                    Some(Click::Text { .. }) => {
                        if let Some(tab) = self.tabs.get_mut(self.tab) {
                            tab.buf.top = (tab.buf.top as isize + by).clamp(0, tab.buf.lines.len().saturating_sub(1) as isize) as usize;
                            let h = self.text_height.max(1);
                            tab.buf.row = tab.buf.row.clamp(tab.buf.top, (tab.buf.top + h - 1).min(tab.buf.lines.len() - 1));
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(l) = list {
                    self.scroll_list(l, by);
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, click: Click, dx: u16, dy: u16, mods: KeyModifiers, again: bool) {
        match click {
            Click::Page(p) => self.go(p),
            Click::Key(code, modifiers) => self.key(KeyEvent::new(code, modifiers)),
            Click::Row(list, i) => self.click_row(list, i, again),
            Click::Section(s) => self.flow_section(s),
            Click::Part(p) => self.admin_part(p),
            Click::DetailTab(t) => self.flow_detail_tab(t),
            Click::Tab(i) => {
                self.show_tab(i);
                self.typing = self.tabs[i].runs.is_none() && self.show != Show::Graph;
            }
            Click::Show(s) => self.set_show(s),
            Click::CloseTab(i) => self.ask_close(i),
            Click::Text { top, left, .. } => {
                let Some(tab) = self.tabs.get_mut(self.tab) else { return };
                let b = &mut tab.buf;
                b.mark(mods.contains(KeyModifiers::SHIFT));
                b.row = (top + dy as usize).min(b.lines.len() - 1);
                b.col = (left + dx as usize).min(b.lines[b.row].chars().count());
                self.typing = tab.runs.is_none();
                self.complete = None;
            }
            Click::Scroll(_) => {}
            // Dialogs that ask something keep it until answered; the others close.
            Click::Outside => {
                if !matches!(self.modal, Some(Modal::Approve { .. } | Modal::DryRun { .. } | Modal::Input { .. } | Modal::Confirm { .. } | Modal::Call(_))) {
                    self.modal = None;
                }
            }
        }
    }

    /// A row clicked: picked, and opened when clicked again.
    fn click_row(&mut self, list: List, i: usize, again: bool) {
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        match list {
            List::Page(p) => {
                self.sel[index(p)] = i;
                self.page = p;
                if again {
                    self.page_key(enter);
                }
            }
            List::Section | List::Detail => {
                self.flow_pick(list, i);
                if again && list == List::Section {
                    self.flow_key(enter);
                }
            }
            List::Complete => {
                if let Some(c) = self.complete.as_mut() {
                    c.sel = i;
                }
                self.take_completion(Some(i));
            }
            List::Palette => {
                let Some(Modal::Palette { query, .. }) = &self.modal else { return };
                let q = query.clone();
                self.palette_pick(&q, i);
            }
            List::Restore => {
                if let Some(Modal::Restore { sel, .. }) = self.modal.as_mut() {
                    *sel = i;
                }
                self.restore_key(enter);
            }
            List::Call => {
                if let Some(Modal::Call(f)) = self.modal.as_mut() {
                    f.pick(i);
                    f.field = 0;
                }
            }
            List::Problems => {
                let Some(tab) = self.tabs.get_mut(self.tab) else { return };
                if let Some(m) = tab.check.mistakes.get(i) {
                    let (line, col) = (m.span.start.line, m.span.start.col);
                    tab.buf.mark(false);
                    tab.buf.go(line, col);
                    self.typing = true;
                }
            }
            List::Runs => {
                if let Some(runs) = self.tabs.get_mut(self.tab).and_then(|t| t.runs.as_mut()) {
                    runs.sel = i.min(runs.entries.len().saturating_sub(1));
                    self.reload_run(self.tab);
                }
            }
            List::Timeline => {
                if let Some(runs) = self.tabs.get_mut(self.tab).and_then(|t| t.runs.as_mut()) {
                    runs.step = i.min(runs.steps().len().saturating_sub(1));
                }
            }
            List::Graph | List::GraphPane => self.tab_graph_pick(list, i, again),
            List::Admin | List::AdminPane => self.admin_pick(list, i, again),
            List::Dialog => {}
        }
    }

    fn scroll_list(&mut self, list: List, by: isize) {
        match list {
            List::Page(p) => self.select(p, by),
            List::Section | List::Detail => self.flow_scroll(list, by),
            List::Complete => {
                if let Some(c) = self.complete.as_mut() {
                    c.sel = (c.sel as isize + by).clamp(0, c.items.len() as isize - 1) as usize;
                }
            }
            List::Palette => {
                if let Some(Modal::Palette { sel, .. }) = self.modal.as_mut() {
                    *sel = (*sel as isize + by).max(0) as usize;
                }
            }
            List::Runs => {
                if let Some(runs) = self.tabs.get_mut(self.tab).and_then(|t| t.runs.as_mut()) {
                    runs.sel = (runs.sel as isize + by).clamp(0, runs.entries.len().saturating_sub(1) as isize) as usize;
                    self.reload_run(self.tab);
                }
            }
            List::Timeline => {
                if let Some(runs) = self.tabs.get_mut(self.tab).and_then(|t| t.runs.as_mut()) {
                    runs.step = (runs.step as isize + by).clamp(0, runs.steps().len().saturating_sub(1) as isize) as usize;
                }
            }
            List::Graph | List::GraphPane => self.tab_graph_scroll(list, by),
            List::Admin | List::AdminPane => self.admin_scroll(list, by),
            _ => {
                if let Some(Modal::Lines { scroll, .. } | Modal::Help { scroll } | Modal::Approve { scroll, .. } | Modal::DryRun { scroll, .. }) = self.modal.as_mut() {
                    *scroll = (*scroll as isize + by).max(0) as usize;
                }
            }
        }
    }
}

fn load_run(c: &Client, runs: &mut Runs) {
    runs.step = 0;
    runs.view = runs.entries.get(runs.sel).map(|e| c.run_view(&runs.slug, e));
}

/// The position the language counts for a tab's cursor.
pub(crate) fn cursor(tab: &Tab) -> Pos {
    let (line, col) = tab.buf.pos();
    Pos { line, col }
}
