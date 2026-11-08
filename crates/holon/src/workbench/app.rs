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
use super::review::{self, Hunk, Review};
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
        Check { mistakes: editing::mistakes(text), colours: review::colours(text), plan: agent::plan(text) }
    }
}

/// A tab's "View runs": the deployment's runs, and one of them over its version's text.
/// A deploy staged, reviewed, ready to approve.
pub(crate) struct Staging {
    pub(crate) slug: String,
    pub(crate) plan: agent::Plan,
    pub(crate) staged: Staged,
    pub(crate) text: String,
    pub(crate) review: Review,
    /// What it may do while it runs, as a line each, and which of them are new.
    pub(crate) may: Vec<String>,
    pub(crate) new: Vec<bool>,
}

/// The Approvals page's pick, beside its list: what approving it does.
pub(crate) struct Approval {
    /// Which wait it is: its flow, and what it waits for.
    pub(crate) of: (String, String),
    pub(crate) what: Pending,
    /// The review has the keys (→ gives them, esc or ← takes them back): ↑ ↓ go through it.
    pub(crate) keys: bool,
}

pub(crate) enum Pending {
    /// A version to deploy, its review.
    Deploy(Box<Staging>),
    /// A flow paused or stopped: enter runs it again.
    Start,
    /// Something a person does elsewhere, said.
    Said(String),
    /// It can't be reviewed: why.
    Failed(String),
}

pub(crate) struct Runs {
    pub(crate) slug: String,
    /// Newest first.
    pub(crate) entries: Vec<Value>,
    pub(crate) sel: usize,
    pub(crate) view: Option<Result<agent::RunView, String>>,
    /// Which noted line the view is at, among those with notes.
    pub(crate) step: usize,
    /// What the bottom panel's right side shows of the run.
    pub(crate) pane: RunPane,
}

/// The right side of a run's panel, as the web editor's: what it read and did, why it failed,
/// and when each statement ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RunPane {
    #[default]
    Details,
    Errors,
    Timeline,
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

    /// Why the run picked failed: its error, and each line that gave nothing for a reason (once
    /// a reason: what it held up says the same). Empty when it didn't.
    pub(crate) fn errors(&self) -> Vec<(Option<u32>, String)> {
        let mut out = vec![];
        if let Some(e) = self.entries.get(self.sel).and_then(|e| e.get("error")).and_then(Value::as_str) {
            out.push((None, e.to_string()));
        }
        if let Some(Ok(v)) = &self.view {
            let mut said = std::collections::HashSet::new();
            for (line, notes) in &v.notes {
                for n in notes {
                    let Some(at) = n.find(" gave nothing: ") else { continue };
                    let why = n[n[..at].rfind(": ").map(|i| i + 2).unwrap_or(0)..].to_string();
                    if said.insert(why.clone()) {
                        out.push((Some(*line), why));
                    }
                }
            }
        }
        out
    }

    /// The panes there are: Errors only for a run that failed.
    pub(crate) fn panes(&self) -> Vec<RunPane> {
        let mut panes = vec![RunPane::Details];
        if !self.errors().is_empty() {
            panes.push(RunPane::Errors);
        }
        panes.push(RunPane::Timeline);
        panes
    }

    /// The next pane (or the one before), round the ends.
    pub(crate) fn next_pane(&mut self, by: isize) {
        let panes = self.panes();
        let at = panes.iter().position(|p| *p == self.pane).unwrap_or(0) as isize;
        self.pane = panes[(at + by).rem_euclid(panes.len() as isize) as usize];
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
    /// What the inspector shows: the name last at the cursor, or just before it as it is typed.
    pub(crate) inspected: Option<Inspected>,
}

/// A name the inspector shows: what it is, the grammar it is written by, a sample's lines.
pub(crate) struct Inspected {
    pub(crate) word: String,
    /// Markdown, as a hover says it.
    pub(crate) hover: String,
    pub(crate) usage: Option<holon_author::usage::Usage>,
}

impl Tab {
    pub(crate) fn new(name: &str, text: &str, path: Option<PathBuf>, slug: Option<String>) -> Tab {
        let buf = Buffer::new(text);
        let kept = buf.text();
        Tab { name: name.to_string(), path, slug, check: Check::of(&kept), kept, buf, edited: None, deployed: None, runs: None, graph: None, draft: false, inspected: None }
    }

    pub(crate) fn dirty(&self) -> bool {
        self.buf.text() != self.kept
    }

    /// Read the name at the cursor, or the one just before it (a name being typed ends where
    /// the cursor is), for the inspector; it keeps the last one while the cursor is elsewhere.
    pub(crate) fn inspect(&mut self) {
        let text = self.buf.text();
        let at = cursor(self);
        let just_before = Pos { line: at.line, col: at.col.saturating_sub(1) };
        let Some((h, pos)) = [at, just_before].into_iter().filter(|p| p.col >= 1).find_map(|p| editing::hover(&text, p).map(|h| (h, p))) else { return };
        let (from, to) = (h.span.start, h.span.end);
        let word: String = text.lines().nth(from.line as usize - 1).unwrap_or_default().chars().skip(from.col as usize - 1).take(if to.line == from.line { (to.col - from.col) as usize } else { 40 }).collect();
        if self.inspected.as_ref().is_some_and(|i| i.word == word && i.hover == h.text) {
            return;
        }
        self.inspected = Some(Inspected { word, hover: h.text, usage: holon_author::usage::usage(&text, pos) });
    }

    /// The deployment it deploys as: a file's is the flow its text names (renamed, it deploys as
    /// the new name); one opened from the server is that one, until its text names another flow.
    pub(crate) fn target(&self) -> Option<String> {
        match self.check.plan.as_ref() {
            Ok(p) => Some(self.deploys_as(&p.name)),
            Err(_) => self.slug.clone(),
        }
    }

    /// The deployment it deploys as, its text naming the flow `named`.
    pub(crate) fn deploys_as(&self, named: &str) -> String {
        let renamed = self.path.is_some() || flow_name(&self.kept).is_some_and(|then| then != named);
        match &self.slug {
            Some(slug) if !renamed => slug.clone(),
            _ => named.to_string(),
        }
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
    /// A flow's data (its state, collections, databases): purple.
    Data,
    /// A call outside it (a request, a file, a channel, another flow): teal.
    Outside,
}

pub(crate) type Said = (Tone, String);

pub(crate) enum Modal {
    /// A deploy, staged: what it asks for, for a person to approve by deploying it, and what it
    /// changes, in the text deployed now and in what it touches.
    Approve { slug: String, plan: Box<agent::Plan>, staged: Staged, text: String, tab: usize, review: Review },
    /// Lines to read: a run explained, a cell's history, an answer.
    Lines { title: String, lines: Vec<Said>, scroll: usize },
    Confirm { title: String, lines: Vec<String>, then: Then },
    Help { scroll: usize },
    /// Everything there is to do and open, by name.
    Palette { query: String, sel: usize },
    /// Going back to a point: its code, its state, or both.
    Restore { slug: String, point: Point, sel: usize },
    /// What a restore would do, as its dry run answers: confirming it restores.
    DryRun { slug: String, point: Point, code: bool, state: bool, review: Review, needs: Vec<String>, refused: bool },
    /// Sending a deployment a request, running an action, firing a clock.
    Call(CallForm),
    /// A line typed in: a secret's value (shown as dots), a session's number; `at` is where
    /// typing goes, a character's place in it.
    Input { title: String, lines: Vec<String>, value: String, at: usize, masked: bool, then: Ask },
    /// Where a secret's value comes from: typed here, a `HOLON_SECRET_…` name the server reads,
    /// or a Vault field.
    SecretSource(super::flow::SecretChoice),
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

/// A line typed outside a dialog of its own (those keep their own cursor): one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Prompt,
    /// A page's `/` filter, by the page's place.
    Filter(usize),
    Shell,
    Palette,
    /// The editor's find bar.
    Find,
    /// The call dialog's field, by number.
    Call(usize),
}

impl Modal {
    /// A line to type in, the cursor at its end.
    pub(crate) fn input(title: String, lines: Vec<String>, value: String, masked: bool, then: Ask) -> Modal {
        let at = value.chars().count();
        Modal::Input { title, lines, value, at, masked, then }
    }
}

pub(crate) enum Ask {
    Secret(String, String),
    /// A secret read from somewhere else: `env:` or `vault:` and what is typed after it.
    SecretFrom(String, String, &'static str),
    Admin(super::admin::AdminAsk),
    /// A file to save tab `0` to (a draft's first save).
    SaveAs(usize),
}

/// The line at the bottom: what just happened.
pub(crate) struct Note {
    pub(crate) text: String,
    pub(crate) error: bool,
    /// Something to know, not something done: drawn dim, without a mark.
    pub(crate) quiet: bool,
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
    /// Where a secret comes from: its choices.
    SecretSource,
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
    /// A review's hunks and folds: a click opens or closes one.
    Review,
    /// The editor's Home tab.
    Home,
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
    /// The graph's switch: every node a row, or the statements with their nodes under each.
    Every,
    /// A tab of a run's panel.
    RunPane(RunPane),
    /// Behind a dialog: a click there closes it, as esc does.
    Outside,
    /// One of the flows page's chips: which flows it lists.
    Shown(Shown),
    /// The editor's Home tab.
    Home,
    /// An input dialog's line, its first character shown being the `from`th.
    Input { from: usize },
}

/// A row of the editor's Home tab.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HomeRow {
    Draft,
    Library,
    /// A tab closed lately, by its place in `closed`.
    Closed(usize),
    /// A flow on the server.
    Flow(String),
}

/// The `:` commands: each, what it takes, what it does (`:` suggests them as they are typed).
pub(crate) const COMMANDS: [(&str, &str, &str); 20] = [
    ("theme", "[light|dark]", "colours for a light terminal, or a dark one (HOLON_THEME=light)"),
    ("open", "<file|slug>", "open a file, or a flow from the server, in a tab"),
    ("new", "[sample]", "a new draft, or a copy of a sample"),
    ("w", "[file]", "save the tab: to its file, to another, else on the server"),
    ("deploy", "", "deploy the tab, approving what it asks"),
    ("fmt", "", "format the tab, as holon fmt writes it"),
    ("close", "", "close the tab"),
    ("flow", "<slug>", "a deployed flow's page"),
    ("call", "[slug]", "call a flow: a route, an action"),
    ("restore", "[slug]", "go back to an earlier version or state"),
    ("sessions", "[slug]", "a flow's sessions: each run of it since a deploy"),
    ("run", "<slug>", "start a paused or stopped flow"),
    ("pause", "<slug>", "pause a flow: its triggers wait"),
    ("stop", "<slug>", "stop a flow"),
    ("server", "<port|url>", "connect to another server"),
    ("refresh", "", "read the server again"),
    ("help", "", "the keys"),
    ("q", "", "quit (asks when tabs are unsaved)"),
    ("q!", "", "quit, unsaved tabs and all"),
    ("!", "[command]", "the shell line, or a command run in this terminal"),
];

/// Which flows the flows page lists, as its chips pick (f goes round them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Shown {
    #[default]
    All,
    /// Deployed and running.
    Live,
    /// Something of it waits for a person.
    Waiting,
    /// Paused or stopped.
    Paused,
}

pub(crate) const SHOWN: [(Shown, &str); 4] = [(Shown::All, "all"), (Shown::Live, "live"), (Shown::Waiting, "waiting"), (Shown::Paused, "paused")];

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
    /// Over a run, each statement's value under its line (`=` hides them, for the code alone).
    pub(crate) values: bool,
    /// The editor has the keyboard: keys type.
    pub(crate) typing: bool,
    /// Code and graph side by side: the graph has the keys (alt+g, a click on it), ↑↓ picking
    /// its rows, the text marking the one picked; esc gives them back to the text.
    pub(crate) graph_keys: bool,
    pub(crate) complete: Option<Complete>,
    /// Find, while open: what is looked for.
    pub(crate) find: Option<String>,
    /// Each list page's `/` filter: only the rows with it in them.
    pub(crate) filter: [String; 7],
    /// The `/` line has the keys: what is typed goes to the page's filter.
    pub(crate) filtering: bool,
    /// Which flows the flows page lists.
    pub(crate) shown: Shown,
    /// The Approvals page's pick, reviewed beside its list.
    pub(crate) approval: Option<Approval>,
    /// Drawn for a light terminal (`:theme`, or HOLON_THEME=light): dark grounds made light.
    pub(crate) light: bool,
    /// The editor's Home tab is shown (it always is with no tab open).
    pub(crate) home: bool,
    pub(crate) home_sel: usize,
    /// The tabs closed lately, newest first, to open again from Home.
    pub(crate) closed: Vec<Tab>,
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
    /// The line being typed (`line_on`): how many of its characters come after the cursor,
    /// none as it opens.
    pub(crate) line_back: usize,
    pub(crate) line_on: Option<Field>,
    /// A command for the event loop to run, with the terminal handed to it.
    pub(crate) run: Option<String>,
    pub(crate) note: Option<Note>,
    pub(crate) quit: bool,
    pub(crate) refreshed: Instant,
    /// The text pane's height when last drawn, for paging.
    pub(crate) text_height: usize,
    /// Where each clickable thing was drawn last, the last drawn on top.
    pub(crate) hits: RefCell<Vec<(Rect, Click)>>,
    /// The panel the keys that work now belong to, as last drawn: they show along its bottom.
    pub(crate) keys_at: RefCell<Option<Rect>>,
    /// What each flow's version waiting for a person asks that it may not do now, by flow,
    /// with when that version was saved: planned once a version, as the lists draw.
    asks: RefCell<std::collections::HashMap<String, (u64, Vec<String>)>>,
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
            flow: FlowView { every: true, ..FlowView::default() },
            admin: Default::default(),
            tabs: vec![],
            tab: 0,
            show: Show::Code,
            values: true,
            typing: false,
            graph_keys: false,
            complete: None,
            find: None,
            filter: Default::default(),
            filtering: false,
            shown: Shown::All,
            approval: None,
            light: std::env::var("HOLON_THEME").is_ok_and(|t| t.eq_ignore_ascii_case("light")),
            home: false,
            home_sel: 0,
            closed: vec![],
            find_seeded: false,
            clipboard: String::new(),
            to_clipboard: None,
            modal: None,
            prompt: None,
            shell: None,
            shell_history: vec![],
            shell_back: None,
            line_back: 0,
            line_on: None,
            run: None,
            note: None,
            quit: false,
            refreshed: Instant::now(),
            text_height: 20,
            hits: RefCell::new(vec![]),
            keys_at: RefCell::new(None),
            asks: RefCell::new(Default::default()),
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
        self.note = Some(Note { text: text.into(), error: false, quiet: false });
    }

    /// A note that only informs (nothing changed, a tip): dim.
    pub(crate) fn tell(&mut self, text: impl Into<String>) {
        self.note = Some(Note { text: text.into(), error: false, quiet: true });
    }

    pub(crate) fn fail(&mut self, text: impl Into<String>) {
        self.note = Some(Note { text: text.into(), error: true, quiet: false });
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
        if path.exists() || arg.ends_with(".holon.ts") {
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
                let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("flow");
                let name = file.strip_suffix(".holon.ts").or_else(|| file.strip_suffix(".ts")).unwrap_or(file);
                format!("import {{ flow }} from \"holon\"\n\nflow(\"{name}\")\n")
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

    /// The editor's Home tab: a draft, the library, the tabs closed lately, the server's flows.
    pub(crate) fn home_rows(&self) -> Vec<HomeRow> {
        let mut rows = vec![HomeRow::Draft, HomeRow::Library];
        rows.extend((0..self.closed.len()).map(HomeRow::Closed));
        let open = |n: &str| self.tabs.iter().any(|t| t.slug.as_deref() == Some(n));
        rows.extend(self.all_flow_names().into_iter().filter(|n| !open(n)).map(HomeRow::Flow));
        rows
    }

    /// Home is what the editor shows: no tab is open, or it was picked.
    pub(crate) fn at_home(&self) -> bool {
        self.tabs.is_empty() || self.home
    }

    pub(crate) fn go_home(&mut self) {
        self.page = Page::Editor;
        self.home = true;
        self.typing = false;
    }

    fn home_key(&mut self, k: KeyEvent) {
        let rows = self.home_rows();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.home_sel = self.home_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.home_sel = (self.home_sel + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Esc | KeyCode::Char(']') if !self.tabs.is_empty() => self.show_tab(self.tab),
            KeyCode::Enter => match rows.get(self.home_sel).cloned() {
                Some(HomeRow::Draft) => self.new_draft(),
                Some(HomeRow::Library) => self.go(Page::Library),
                Some(HomeRow::Closed(i)) if i < self.closed.len() => {
                    let t = self.closed.remove(i);
                    self.add_tab(t);
                }
                Some(HomeRow::Flow(name)) => self.edit(&name),
                _ => {}
            },
            _ => {}
        }
    }

    pub(crate) fn show_tab(&mut self, i: usize) {
        self.home = false;
        self.tab = i;
        self.graph_keys = false;
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
        match latest_text(c, name) {
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
                    self.flow = FlowView { every: self.flow.every, ..FlowView::default() };
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
    /// The flows page's rows: every flow, deployed or saved, by name; those its chip and its
    /// filter let by.
    pub(crate) fn flow_names(&self) -> Vec<String> {
        let mut names = self.all_flow_names();
        let Some(c) = self.client.as_ref() else { return names };
        let waits: Vec<String> = c.waiting().into_iter().map(|(slug, _)| slug).collect();
        let filter = self.filter[index(Page::Flows)].to_lowercase();
        names.retain(|n| {
            let status = c.deployment(n).and_then(|d| d["status"].as_str());
            let shown = match self.shown {
                Shown::All => true,
                Shown::Live => status == Some("running"),
                Shown::Waiting => waits.contains(n),
                Shown::Paused => matches!(status, Some("paused" | "stopped")),
            };
            shown && n.to_lowercase().contains(&filter)
        });
        names
    }

    /// Every flow on the server, deployed or saved, by name.
    pub(crate) fn all_flow_names(&self) -> Vec<String> {
        let Some(c) = self.client.as_ref() else { return vec![] };
        let mut names: Vec<String> = c.deployments.iter().chain(&c.flows).filter_map(|v| v.get("slug").or(v.get("name")).and_then(Value::as_str).map(str::to_string)).collect();
        names.sort();
        names.dedup();
        names
    }

    /// The library's rows: the samples its filter lets by (a name, a title or what one shows).
    pub(crate) fn library(&self) -> Vec<&'static samples::Sample> {
        let filter = self.filter[index(Page::Library)].to_lowercase();
        samples::samples().iter().filter(|s| [s.name, s.title].into_iter().chain(s.shows.iter().copied()).any(|t| t.to_lowercase().contains(&filter))).collect()
    }

    /// A list page's `/` filter, empty when it has none.
    pub(crate) fn filter_of(&self, page: Page) -> &str {
        &self.filter[index(page)]
    }

    /// What the version of `slug` that waits for a person asks that it may not do now, a line
    /// each (`read its state "n"`): none when no version waits. Its saved text is planned once.
    pub(crate) fn asks(&self, slug: &str) -> Vec<String> {
        let Some(c) = self.client.as_ref() else { return vec![] };
        let Some(saved_at) = c.saved(slug).and_then(|f| f["saved_at_epoch_seconds"].as_u64()) else { return vec![] };
        let deployed_at = c.deployment(slug).and_then(|d| d["deployed_at_epoch_seconds"].as_u64());
        if deployed_at.is_some_and(|d| d >= saved_at) {
            return vec![];
        }
        if let Some((_, asks)) = self.asks.borrow().get(slug).filter(|(at, _)| *at == saved_at) {
            return asks.clone();
        }
        let now: Vec<String> = deployed_at.and_then(|_| c.get(&format!("/deployments/{}/manifest", crate::mcp::seg(slug))).ok()).map(|m| client::strings(&m, "items")).unwrap_or_default();
        let asks: Vec<String> = c.saved_text(slug).ok().and_then(|t| agent::plan(&t).ok()).map(|p| p.may.iter().filter(|m| !now.contains(m)).map(|m| agent::describe(m)).collect()).unwrap_or_default();
        self.asks.borrow_mut().insert(slug.to_string(), (saved_at, asks.clone()));
        asks
    }

    /// The pages `/` filters in place; elsewhere it finds anything.
    pub(crate) fn filters(&self, page: Page) -> bool {
        matches!(page, Page::Flows | Page::Library) || (page == Page::Flow && self.flow.section == super::flow::Section::Runs)
    }

    /// A key while the `/` line is open: it types the filter, the list narrowing as it does.
    fn filter_key(&mut self, k: KeyEvent) {
        let at = index(self.page);
        match k.code {
            KeyCode::Esc => {
                self.filter[at].clear();
                self.filtering = false;
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Backspace if self.filter[at].is_empty() => self.filtering = false,
            KeyCode::Up if self.page == Page::Flow => return self.flow_move(-1),
            KeyCode::Down if self.page == Page::Flow => return self.flow_move(1),
            KeyCode::Up => return self.select(self.page, -1),
            KeyCode::Down => return self.select(self.page, 1),
            _ => {
                if !edit_typed(&mut self.filter[at], &mut self.line_back, k) {
                    return;
                }
            }
        }
        self.refilter();
    }

    /// The list a filter narrows goes back to its first row.
    fn refilter(&mut self) {
        let at = index(self.page);
        self.sel[at] = 0;
        if self.page == Page::Flow {
            self.flow_pick(List::Section, 0);
        }
    }

    /// The flows page's next chip.
    pub(crate) fn show_flows(&mut self, s: Shown) {
        self.shown = s;
        self.sel[index(Page::Flows)] = 0;
    }

    pub(crate) fn waiting(&self) -> Vec<(String, String)> {
        self.client.as_ref().map(Client::waiting).unwrap_or_default()
    }

    fn rows(&self, page: Page) -> usize {
        match page {
            Page::Overview => self.client.as_ref().map_or(0, |c| c.deployments.len()),
            Page::Approvals => self.waiting().len(),
            Page::Flows => self.flow_names().len(),
            Page::Library => self.library().len(),
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
            self.modal = Some(Modal::input(
                format!("Save {} as", tab.name),
                vec![format!("A file, from {here} (~ is your home folder):"), "Enter writes it there; esc leaves it a draft.".into()],
                format!("{name}.holon.ts"),
                false,
                Ask::SaveAs(i),
            ));
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
        let name = tab.deploys_as(&plan.name);
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
        let slug = tab.deploys_as(&plan.name);
        let before = tab.slug.clone().filter(|s| *s != slug);
        let Some(c) = self.client.as_ref() else {
            return self.fail(format!("no server to deploy to: {}", self.offline.clone().unwrap_or_else(|| "start holon serve".into())));
        };
        match staging(c, slug, plan, text, before) {
            Ok(s) => self.modal = Some(Modal::Approve { slug: s.slug, plan: Box::new(s.plan), staged: s.staged, text: s.text, tab: i, review: s.review }),
            Err(e) => self.fail(e),
        }
    }

    /// Save what the review shows on the server without deploying it: it waits in Approvals for
    /// whoever may approve it.
    fn propose(&mut self) {
        let Some(Modal::Approve { slug, plan, text, tab, .. }) = self.modal.take() else { return };
        let Some(c) = self.client.as_ref() else { return };
        match c.save(&slug, &plan, &text) {
            Ok(()) => {
                if let Some(t) = self.tabs.get_mut(tab).filter(|t| t.path.is_none()) {
                    t.kept = text;
                    t.slug = Some(slug.clone());
                    t.name = slug.clone();
                }
                self.refresh();
                self.say(format!("saved \"{slug}\", not deployed: it waits in Approvals"));
            }
            Err(e) => self.fail(e),
        }
    }

    /// Deploy what the review shows, approving it as us.
    fn approve(&mut self) {
        let Some(Modal::Approve { slug, plan, staged, text, tab, .. }) = self.modal.take() else { return };
        self.commit_deploy(slug, plan, staged, text, Some(tab));
    }

    /// Deploy what was staged, approving it as us; the tab it was deployed from, if any, keeps it.
    fn commit_deploy(&mut self, slug: String, plan: Box<agent::Plan>, staged: Staged, text: String, tab: Option<usize>) {
        let Some(c) = self.client.as_ref() else { return };
        let needs = staged.needs.len();
        match c.commit(&plan, staged, &text) {
            Ok(()) => {
                if let Some(t) = tab.and_then(|i| self.tabs.get_mut(i)) {
                    t.slug = Some(slug.clone());
                    t.deployed = Some(plan.may.clone());
                    if t.path.is_none() {
                        t.kept = text;
                        t.name = slug.clone();
                    }
                }
                self.refresh();
                let approved = if needs == 0 { String::new() } else { format!(", approving {needs} new permission{} as you", if needs == 1 { "" } else { "s" }) };
                let reach = if plan.routes.is_empty() { started_by(&plan).map(|s| format!("; {s}")).unwrap_or_default() } else { format!("; {}", plan.routes.join(", ")) };
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
            // Kept a while, to open again from Home, as it was.
            let mut t = self.tabs.remove(i);
            t.runs = None;
            t.graph = None;
            // The same file, or the same flow from the server, kept once.
            let same = |c: &Tab| if c.path.is_some() { c.path == t.path } else { c.slug.is_some() && c.slug == t.slug };
            self.closed.retain(|c| !same(c));
            self.closed.insert(0, t);
            self.closed.truncate(8);
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
        let target = tab.target();
        let Some(slug) = target.clone().filter(|s| self.client.as_ref().is_some_and(|c| c.deployment(s).is_some())) else {
            let name = target.map(|t| format!("\"{t}\" ")).unwrap_or_default();
            return self.fail(format!("{name}isn't deployed: deploy it (ctrl+d) to see its runs"));
        };
        let c = self.client.as_ref().unwrap();
        match c.get(&format!("/deployments/{}/log?limit={}", crate::mcp::seg(&slug), client::RUNS)) {
            Ok(log) => {
                let mut entries = log["entries"].as_array().cloned().unwrap_or_default();
                entries.reverse();
                let mut runs = Runs { slug, entries, sel: 0, view: None, step: 0, pane: RunPane::Details };
                load_run(c, &mut runs);
                self.tabs[i].runs = Some(runs);
                self.typing = false;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Show a run's values under its lines, or hide them for the code alone; for every tab.
    pub(crate) fn toggle_values(&mut self) {
        self.values = !self.values;
        self.tell(if self.values { "values shown under their lines: = hides them" } else { "values hidden, the code alone: = shows them" });
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

    /// What the `:` line may become, as it is typed: the commands it starts, else the flows
    /// the command's argument starts; each the whole line, and what it does.
    pub(crate) fn suggestions(&self) -> Vec<(String, &'static str)> {
        let Some(p) = self.prompt.as_deref() else { return vec![] };
        match p.split_once(' ') {
            None => COMMANDS.iter().filter(|(c, _, _)| c.starts_with(p)).map(|(c, args, what)| (if args.is_empty() { c.to_string() } else { format!("{c} ") }, *what)).take(9).collect(),
            Some((cmd, arg)) => {
                let Some((_, args, what)) = COMMANDS.iter().find(|(c, ..)| *c == cmd) else { return vec![] };
                let names: Vec<String> = if args.contains("slug") {
                    self.all_flow_names()
                } else if args.contains("sample") {
                    samples::samples().iter().map(|s| s.name.to_string()).collect()
                } else {
                    return vec![];
                };
                let arg = arg.trim_start();
                names.into_iter().filter(|n| n.starts_with(arg) && n != arg).map(|n| (format!("{cmd} {n}"), *what)).take(9).collect()
            }
        }
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
            ("theme", a) => {
                self.light = match a {
                    "light" => true,
                    "dark" => false,
                    _ => !self.light,
                };
                self.tell(if self.light { "a light theme: :theme dark goes back" } else { "a dark theme: :theme light for a light terminal" });
            }
            _ => self.fail(format!(":{line}? (:help lists the commands; ctrl+p finds anything)")),
        }
    }

    // ── keys ───────────────────────────────────────────────────────────────────────────────

    /// The line being typed now, the one keys go to.
    pub(crate) fn field(&self) -> Option<Field> {
        match &self.modal {
            Some(Modal::Palette { .. }) => Some(Field::Palette),
            Some(Modal::Call(form)) => Some(Field::Call(form.field)),
            Some(_) => None,
            None if self.prompt.is_some() => Some(Field::Prompt),
            None if self.filtering => Some(Field::Filter(index(self.page))),
            None if self.shell.is_some() => Some(Field::Shell),
            None if self.find.is_some() && self.page == Page::Editor => Some(Field::Find),
            None => None,
        }
    }

    /// What the line being typed holds.
    pub(crate) fn field_text(&self) -> Option<&str> {
        Some(match self.field()? {
            Field::Prompt => self.prompt.as_deref()?,
            Field::Filter(i) => &self.filter[i],
            Field::Shell => self.shell.as_deref()?,
            Field::Find => self.find.as_deref()?,
            Field::Palette | Field::Call(_) => match &self.modal {
                Some(Modal::Palette { query, .. }) => query,
                Some(Modal::Call(form)) if form.field > 0 => form.field_value(form.field),
                _ => return None,
            },
        })
    }

    /// A line opened, or another one gone to: its cursor at the end.
    fn field_moved(&mut self) {
        let now = self.field();
        if now != self.line_on {
            self.line_on = now;
            self.line_back = 0;
        }
    }

    /// Text pasted: into the line being typed, at its cursor (its first line), else the text.
    pub(crate) fn paste(&mut self, text: &str) {
        self.field_moved();
        let first = text.lines().next().unwrap_or("");
        let into = |s: &mut String, back: &mut usize, t: &str| {
            for c in t.chars() {
                edit_typed(s, back, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            }
        };
        if let Some(p) = self.prompt.as_mut().or(self.shell.as_mut()) {
            into(p, &mut self.line_back, first);
        } else if self.modal.is_none() && self.filtering {
            into(&mut self.filter[index(self.page)], &mut self.line_back, first);
            self.refilter();
        } else if let Some(Modal::Input { value, at, .. }) = self.modal.as_mut() {
            for c in first.chars() {
                edit_line(value, at, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            }
        } else if let Some(Modal::Palette { query, sel }) = self.modal.as_mut() {
            into(query, &mut self.line_back, first);
            *sel = 0;
        } else if let Some(Modal::Call(form)) = self.modal.as_mut() {
            if form.field == 0 && form.fields() > 0 {
                form.field = 1;
                self.line_back = 0;
                self.line_on = Some(Field::Call(1));
            }
            if let Some(s) = form.focused() {
                into(s, &mut self.line_back, &text.replace(['\r', '\n'], " "));
            }
        } else if self.modal.is_none() && self.page == Page::Editor && let Some(q) = self.find.as_mut() {
            into(q, &mut self.line_back, first);
            self.find_seeded = false;
            let q = q.clone();
            if let Some(tab) = self.tabs.get_mut(self.tab) {
                tab.buf.find(&q, false);
            }
        } else if self.page == Page::Editor && self.typing && let Some(tab) = self.tabs.get_mut(self.tab) {
            tab.buf.insert_str(text);
            tab.edited = Some(Instant::now());
        }
    }

    pub(crate) fn key(&mut self, k: KeyEvent) {
        self.field_moved();
        self.key_in(k);
        self.field_moved();
    }

    fn key_in(&mut self, k: KeyEvent) {
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
        if self.filtering {
            return self.filter_key(k);
        }
        if self.shell.is_some() {
            return self.shell_key(k);
        }
        // ctrl+` opens the shell line from anywhere, typing included (! does where keys aren't text).
        if ctrl && k.code == KeyCode::Char('`') {
            return self.open_shell();
        }
        // A draft in a new tab: ctrl+t, as a browser opens one (ctrl+n too, where the terminal lets it by).
        if ctrl && matches!(k.code, KeyCode::Char('t') | KeyCode::Char('n')) {
            return self.new_draft();
        }
        if ctrl && matches!(k.code, KeyCode::Char('p') | KeyCode::Char('k')) {
            self.modal = Some(Modal::Palette { query: String::new(), sel: 0 });
            return;
        }
        if self.page == Page::Editor && !self.at_home() {
            if ctrl && self.editor_shortcut(k) {
                return;
            }
            // A ctrl letter the editor doesn't take is no page key: ctrl+f mustn't format.
            if ctrl && !self.typing && matches!(k.code, KeyCode::Char(_)) {
                return;
            }
            if k.modifiers.contains(KeyModifiers::ALT) && !k.modifiers.contains(KeyModifiers::CONTROL) {
                match k.code {
                    KeyCode::Char('f' | 'F') => return self.format(self.tab),
                    KeyCode::Char('g' | 'G') if self.show == Show::Both => return self.graph_beside(),
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
            // Find works from the page keys too: it goes back into the text.
            KeyCode::Char('f') if typing || (self.tabs[i].runs.is_none() && self.show != Show::Graph) => {
                self.typing = true;
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
        self.graph_keys = false;
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
            KeyCode::Backspace | KeyCode::Delete => {
                edit_typed(q, &mut self.line_back, k);
                b.anchor = None;
                q.is_empty() || b.find(q, false)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => {
                self.find_seeded = false;
                edit_typed(q, &mut self.line_back, k)
            }
            KeyCode::Char(_) => {
                // A query taken from the selection is replaced by what is typed first.
                if std::mem::take(&mut self.find_seeded) {
                    q.clear();
                    self.line_back = 0;
                }
                edit_typed(q, &mut self.line_back, k);
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
            KeyCode::Char('/') if self.filters(page) => return self.filtering = true,
            KeyCode::Esc if self.filters(page) && !self.filter[index(page)].is_empty() => {
                self.filter[index(page)].clear();
                return self.refilter();
            }
            KeyCode::Char('f') if page == Page::Flows => {
                let next = SHOWN.iter().position(|(s, _)| *s == self.shown).map_or(0, |i| (i + 1) % SHOWN.len());
                return self.show_flows(SHOWN[next].0);
            }
            KeyCode::Char('/') if page != Page::Flow => return self.modal = Some(Modal::Palette { query: String::new(), sel: 0 }),
            KeyCode::Char('r') if page != Page::Editor => {
                if self.client.is_none() {
                    self.connect();
                } else {
                    self.refresh();
                }
                return self.tell("read the server again");
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
        if page == Page::Approvals && self.approval_key(k) {
            return;
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
                    && let Some(s) = self.library().get(sel)
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
        if to == Page::Editor && !self.at_home() && self.tabs[self.tab].runs.is_none() && self.show != Show::Graph {
            self.typing = true;
        }
    }

    /// Do what a deployment waits on a person for: start it, or review and deploy its text.
    /// Review the Approvals page's pick, once for each pick: staged as a deploy would be.
    pub(crate) fn sync_approval(&mut self) {
        let waiting = self.waiting();
        let Some(of) = waiting.get(self.sel[index(Page::Approvals)].min(waiting.len().saturating_sub(1))).cloned() else {
            self.approval = None;
            return;
        };
        if self.approval.as_ref().is_some_and(|a| a.of == of) {
            return;
        }
        let Some(c) = self.client.as_ref() else { return };
        let (slug, wait) = (&of.0, &of.1);
        let what = if wait.starts_with("to start it") {
            Pending::Start
        } else if wait.starts_with("to free its port") {
            Pending::Said(format!("Its port is taken: {wait}. Stop what holds it, then deploy it again."))
        } else {
            let staged = latest_text(c, slug).and_then(|text| {
                let plan = agent::plan(&text)?;
                staging(c, slug.clone(), plan, text, None)
            });
            match staged {
                Ok(s) => Pending::Deploy(Box::new(s)),
                Err(e) => Pending::Failed(e),
            }
        };
        self.approval = Some(Approval { of, what, keys: false });
    }

    /// A key on the Approvals page that its review takes: true when taken.
    fn approval_key(&mut self, k: KeyEvent) -> bool {
        let n = self.waiting().len();
        let at = index(Page::Approvals);
        let Some(a) = self.approval.as_mut() else { return false };
        if a.keys {
            if let Pending::Deploy(s) = &mut a.what
                && s.review.key(k)
            {
                return true;
            }
            match k.code {
                KeyCode::Esc | KeyCode::Left => a.keys = false,
                KeyCode::Enter => self.approve_picked(),
                _ => return false,
            }
            return true;
        }
        match k.code {
            KeyCode::Right | KeyCode::Tab if matches!(a.what, Pending::Deploy(_)) => a.keys = true,
            KeyCode::Char(']') if n > 0 => self.sel[at] = (self.sel[at] + 1) % n,
            KeyCode::Char('[') if n > 0 => self.sel[at] = (self.sel[at] + n - 1) % n,
            KeyCode::Enter => self.approve_picked(),
            KeyCode::Char('e') => {
                let slug = a.of.0.clone();
                self.edit(&slug);
            }
            _ => return false,
        }
        true
    }

    /// Do what the Approvals page's pick waits for: deploy it, approving what it asks, or run it.
    fn approve_picked(&mut self) {
        // What a person does elsewhere stays picked, said again.
        if let Some(Approval { what: Pending::Said(why) | Pending::Failed(why), .. }) = &self.approval {
            let why = why.clone();
            return self.fail(why);
        }
        let Some(a) = self.approval.take() else { return };
        match a.what {
            Pending::Deploy(s) => {
                let Staging { slug, plan, staged, text, .. } = *s;
                self.commit_deploy(slug, Box::new(plan), staged, text, None);
            }
            Pending::Start => self.resolve(&a.of.0, &a.of.1),
            Pending::Said(_) | Pending::Failed(_) => {}
        }
    }

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
        if self.at_home() {
            return self.home_key(k);
        }
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
                // The run before (older) and after (newer), as a player's , and .
                KeyCode::Char(',') => {
                    runs.sel = (runs.sel + 1).min(runs.entries.len().saturating_sub(1));
                    self.reload_run(i);
                }
                KeyCode::Char('.') => {
                    runs.sel = runs.sel.saturating_sub(1);
                    self.reload_run(i);
                }
                KeyCode::Char('n') | KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => runs.go(1),
                KeyCode::Char('N') | KeyCode::Left | KeyCode::Char('h') => runs.go(-1),
                KeyCode::Home => runs.step = 0,
                KeyCode::End => runs.step = runs.steps().len().saturating_sub(1),
                KeyCode::Tab => runs.next_pane(1),
                KeyCode::BackTab => runs.next_pane(-1),
                KeyCode::Char('=') => self.toggle_values(),
                KeyCode::Char('b') => self.next_show(),
                _ => {}
            }
            return;
        }
        if self.show == Show::Graph {
            return self.tab_graph_key(k);
        }
        if self.beside_has_keys() {
            return self.beside_key(k);
        }
        match k.code {
            KeyCode::Enter | KeyCode::Char('i') => self.typing = true,
            KeyCode::Tab if self.show == Show::Both => self.graph_beside(),
            KeyCode::Char('[') => self.show_tab((i + self.tabs.len() - 1) % self.tabs.len()),
            KeyCode::Char(']') => self.show_tab((i + 1) % self.tabs.len()),
            KeyCode::Char('w') => self.ask_close(i),
            KeyCode::Char('s') => self.save(i, None),
            KeyCode::Char('d') => self.deploy(i),
            KeyCode::Char('v') => self.toggle_runs(i),
            KeyCode::Char('b') => self.next_show(),
            KeyCode::Char('a') if self.show == Show::Both => self.tab_every(),
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
            KeyCode::Backspace if p.is_empty() => self.prompt = None,
            // The first suggestion, taken.
            KeyCode::Tab => {
                if let Some((line, _)) = self.suggestions().into_iter().next() {
                    self.prompt = Some(line);
                    self.line_back = 0;
                }
            }
            _ => {
                edit_typed(p, &mut self.line_back, k);
            }
        }
    }

    fn modal_key(&mut self, k: KeyEvent) {
        let Some(modal) = self.modal.as_mut() else { return };
        let page = |by: isize, at: &mut usize, n: usize| *at = (*at as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        match modal {
            Modal::Approve { review, .. } => match k.code {
                _ if review.key(k) => {}
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('a') => self.approve(),
                KeyCode::Char('s') => self.propose(),
                KeyCode::Esc | KeyCode::Char('x') => {
                    self.modal = None;
                    self.tell("not deployed");
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
                KeyCode::Enter => {
                    let (q, s) = (query.clone(), *sel);
                    self.palette_pick(&q, s);
                }
                _ => {
                    let was = query.clone();
                    edit_typed(query, &mut self.line_back, k);
                    if *query != was {
                        *sel = 0;
                    }
                }
            },
            Modal::Restore { .. } => self.restore_key(k),
            Modal::SecretSource(_) => self.secret_source_key(k),
            Modal::DryRun { .. } => self.dry_run_key(k),
            Modal::Call(_) => self.call_key(k),
            Modal::Input { value, at, .. } => match k.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Enter => self.answer_input(),
                _ => {
                    edit_line(value, at, k);
                }
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
                self.field_moved();
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
            Click::Every if self.page == Page::Flow => self.flow_every(),
            Click::Every => self.tab_every(),
            Click::Shown(s) => self.show_flows(s),
            Click::Home => self.go_home(),
            Click::RunPane(p) => {
                if let Some(runs) = self.tabs.get_mut(self.tab).and_then(|t| t.runs.as_mut()) {
                    runs.pane = p;
                }
            }
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
            Click::Input { from } => {
                if let Some(Modal::Input { value, at, .. }) = self.modal.as_mut() {
                    *at = (from + dx as usize).min(value.chars().count());
                } else if let Some(n) = self.field_text().map(|s| s.chars().count()) {
                    self.line_back = n - (from + dx as usize).min(n);
                }
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
            List::SecretSource => {
                if let Some(Modal::SecretSource(c)) = self.modal.as_mut() {
                    c.sel = i.min(c.choices().len().saturating_sub(1));
                }
                self.secret_source_key(enter);
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
            List::Review => {
                if let Some(Modal::Approve { review, .. } | Modal::DryRun { review, .. }) = self.modal.as_mut() {
                    review.click(i);
                } else if let Some(Approval { what: Pending::Deploy(s), keys, .. }) = self.approval.as_mut() {
                    *keys = true;
                    s.review.click(i);
                }
            }
            List::Dialog => {}
            List::Home => {
                self.home_sel = i;
                if again {
                    self.home_key(enter);
                }
            }
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
                if let Some(Modal::Lines { scroll, .. } | Modal::Help { scroll } | Modal::Approve { review: Review { scroll, .. }, .. } | Modal::DryRun { review: Review { scroll, .. }, .. }) = self.modal.as_mut() {
                    *scroll = (*scroll as isize + by).max(0) as usize;
                } else if let (None, Some(Approval { what: Pending::Deploy(s), .. })) = (&self.modal, self.approval.as_mut()) {
                    s.review.scroll = (s.review.scroll as isize + by).max(0) as usize;
                }
            }
        }
    }
}

/// What a deploy changes, to look at before approving it: its code against the text deployed now
/// (none for a first deploy), what it touches, its routes; and what deploying it approves.
/// What starts a flow's runs besides requests and a person: its clocks and file handlers (`every
/// 5s`, `on file inbox`), as its plan's handlers are labeled; none when it has neither.
pub(crate) fn started_by(plan: &agent::Plan) -> Option<String> {
    let labels: Vec<&str> = plan.document["trace"].as_object().into_iter().flat_map(|t| t.keys()).map(String::as_str).filter(|l| l.starts_with("every ") || l.starts_with("on file ")).collect();
    (!labels.is_empty()).then(|| labels.join(", "))
}

fn deploy_review(slug: &str, plan: &agent::Plan, staged: &Staged, now: Option<&str>, text: &str, server: &str) -> Review {
    let mut review = Review { top: vec![(Tone::Dim, format!("Deploying \"{slug}\" ({}) to {server}.", &plan.unit[..12]))], ..Review::default() };
    if let Some(now) = now {
        review.hunks.extend(review::code("Its code: deployed now → this text", now, text, &staged.touches));
    }
    let touched = review::touched(&serde_json::json!({ "touches": staged.touches, "settings": staged.settings }));
    if touched.is_empty() {
        review.top.push((Tone::Dim, "It only computes: it touches nothing outside a run.".into()));
    }
    review.hunks.extend(touched);
    // Asked beside its code (an agent's new site to answer pages from).
    let listed: Vec<&str> = staged.touches.as_array().into_iter().flatten().flat_map(|r| r["uses"].as_array().into_iter().flatten()).filter_map(|u| u["item"].as_str()).collect();
    let also = review::rows('a', staged.needs.iter().filter(|n| !listed.contains(&n.as_str())).map(|n| agent::describe(n)));
    if !also.is_empty() {
        review.hunks.push(Hunk::new("Also asked", vec![(Tone::Warn, format!("{} to approve", also.len()))], also, true));
    }
    if !plan.routes.is_empty() {
        review.hunks.push(Hunk::new("Its routes", vec![(Tone::Dim, plan.routes.len().to_string())], review::rows(' ', plan.routes.iter().cloned()), true));
    }
    review.foot.push(match staged.needs.len() {
        0 => (Tone::Good, "Nothing new to approve: its grant covers all of this.".into()),
        n => (Tone::Warn, format!("You approve the {n} marked +, as you, by deploying it.")),
    });
    review
}

/// A flow's latest text: a version saved since it was deployed, else the one deployed, else the
/// one saved.
fn latest_text(c: &Client, name: &str) -> Result<String, String> {
    let deployed_at = c.deployment(name).and_then(|d| d["deployed_at_epoch_seconds"].as_u64());
    let saved_at = c.saved(name).and_then(|f| f["saved_at_epoch_seconds"].as_u64());
    match (deployed_at, saved_at) {
        (Some(d), Some(s)) if s > d => c.saved_text(name),
        (Some(_), _) => c.text(name, None),
        (None, Some(_)) => c.saved_text(name),
        (None, None) => Err(format!("nothing named \"{name}\" is deployed or saved")),
    }
}

/// A deploy staged and reviewed: what its dialog, or the Approvals page, shows and approves.
fn staging(c: &Client, slug: String, plan: agent::Plan, text: String, before: Option<String>) -> Result<Staging, String> {
    let now = c.deployment(&slug).and_then(|_| c.text(&slug, None).ok());
    let staged = c.stage(&plan, &slug)?;
    let mut review = deploy_review(&slug, &plan, &staged, now.as_deref(), &text, &c.server());
    // Renamed: it deploys under its new name, and what it was deployed as stays.
    if let Some(before) = before.filter(|_| now.is_none()) {
        review.top.push((Tone::Warn, format!("Its text names it \"{slug}\" now: it deploys as a new flow. \"{before}\", which this tab deployed before, stays as it is.")));
    }
    // Another flow listening on its port: one of the two can't.
    if let Some(other) = plan.port.and_then(|port| c.deployments.iter().find(|d| d["slug"] != slug.as_str() && d["port"].as_u64() == Some(port.into()))) {
        review.top.push((Tone::Bad, format!("Port {} is \"{}\"'s: this one can't listen there while it runs (change the port line, or stop it).", plan.port.unwrap_or_default(), other["slug"].as_str().unwrap_or_default())));
    }
    // What it may do while it runs, a new permission marked: the "if approved" of it.
    let deployed: Vec<String> = c.deployment(&slug).and_then(|_| c.get(&format!("/deployments/{}/manifest", crate::mcp::seg(&slug))).ok()).map(|m| client::strings(&m, "items")).unwrap_or_default();
    let may: Vec<String> = plan.may.iter().map(|m| agent::describe(m)).collect();
    let new: Vec<bool> = plan.may.iter().map(|m| !deployed.contains(m)).collect();
    Ok(Staging { slug, plan, staged, text, review, may, new })
}

/// The name a text gives its flow, `flow("name", …)`, as typed, whether or not it compiles.
fn flow_name(text: &str) -> Option<&str> {
    let rest = text.lines().find_map(|l| l.trim_start().strip_prefix("flow("))?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    rest[1..].split(quote).next()
}

fn load_run(c: &Client, runs: &mut Runs) {
    runs.step = 0;
    runs.view = runs.entries.get(runs.sel).map(|e| c.run_view(&runs.slug, e));
    // A run that failed opens on why, as the web editor's does; one that didn't leaves Errors.
    if !runs.errors().is_empty() {
        runs.pane = RunPane::Errors;
    } else if runs.pane == RunPane::Errors {
        runs.pane = RunPane::Details;
    }
}

/// A key on a line being typed, the cursor at `at` (a character's place, kept within it): it
/// moves (left, right, home, end), deletes on either side of it, or types there. False when
/// the key isn't one of those.
pub(crate) fn edit_line(value: &mut String, at: &mut usize, k: KeyEvent) -> bool {
    let n = value.chars().count();
    *at = (*at).min(n);
    let byte = |s: &str, i: usize| s.char_indices().nth(i).map_or(s.len(), |(b, _)| b);
    match k.code {
        KeyCode::Left => *at = at.saturating_sub(1),
        KeyCode::Right => *at = (*at + 1).min(n),
        KeyCode::Home => *at = 0,
        KeyCode::End => *at = n,
        KeyCode::Backspace => {
            if *at > 0 {
                value.remove(byte(value, *at - 1));
                *at -= 1;
            }
        }
        KeyCode::Delete => {
            if *at < n {
                value.remove(byte(value, *at));
            }
        }
        KeyCode::Char(c) => {
            value.insert(byte(value, *at), c);
            *at += 1;
        }
        _ => return false,
    }
    true
}

/// `edit_line` on a line whose cursor is counted from its end (`back` characters after it).
pub(crate) fn edit_typed(value: &mut String, back: &mut usize, k: KeyEvent) -> bool {
    let n = value.chars().count();
    let mut at = n - (*back).min(n);
    let done = edit_line(value, &mut at, k);
    *back = value.chars().count() - at;
    done
}

/// The position the language counts for a tab's cursor.
pub(crate) fn cursor(tab: &Tab) -> Pos {
    let (line, col) = tab.buf.pos();
    Pos { line, col }
}
