//! What `holon editor` holds and what each key does: the pages, the open flow, the tabs, and the
//! dialog or prompt in front of them. Drawing it is ui.rs's.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use holon_author::editing::{self, Class, Mistake};
use holon_author::lang::ast::Pos;
use holon_author::{agent, samples};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use super::client::{self, Client, Detail};
use super::text::Buffer;
use crate::deploycmd::Staged;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Overview,
    Approvals,
    Flows,
    /// One deployment: what it may do, its runs, its timeline.
    Flow,
    Editor,
    Library,
}

/// The pages, in the sidebar's order: each one's key is its place.
pub(crate) const PAGES: [(Page, &str); 6] =
    [(Page::Overview, "Overview"), (Page::Approvals, "Approvals"), (Page::Flows, "Flows"), (Page::Flow, "Flow"), (Page::Editor, "Editor"), (Page::Library, "Library")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlowTab {
    Status,
    Runs,
    History,
}

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
    /// The noted lines (1-based), in order.
    pub(crate) fn noted(&self) -> Vec<u32> {
        match &self.view {
            Some(Ok(v)) => v.notes.keys().copied().collect(),
            _ => vec![],
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
}

impl Tab {
    pub(crate) fn new(name: &str, text: &str, path: Option<PathBuf>, slug: Option<String>) -> Tab {
        let buf = Buffer::new(text);
        let kept = buf.text();
        Tab { name: name.to_string(), path, slug, check: Check::of(&kept), kept, buf, edited: None, deployed: None, runs: None }
    }

    pub(crate) fn dirty(&self) -> bool {
        self.buf.text() != self.kept
    }

    /// The deployment it deploys as: its own, else the flow's name.
    pub(crate) fn target(&self) -> Option<String> {
        self.slug.clone().or_else(|| self.check.plan.as_ref().ok().map(|p| p.name.clone()))
    }
}

pub(crate) enum Modal {
    /// A deploy, staged: what it asks for, for a person to approve by deploying it.
    Approve { slug: String, plan: Box<agent::Plan>, staged: Staged, text: String, tab: usize },
    /// One run, as its version's text with what each line did.
    Explain { title: String, lines: Vec<String>, scroll: usize },
    Confirm { title: String, lines: Vec<String>, then: Then },
    Help { scroll: usize },
}

pub(crate) enum Then {
    Lifecycle(String, &'static str),
    Quit,
    Close(usize),
}

/// The line at the bottom: what just happened.
pub(crate) struct Note {
    pub(crate) text: String,
    pub(crate) error: bool,
}

pub(crate) struct App {
    /// `--server`, to connect again.
    pub(crate) server_arg: Option<String>,
    pub(crate) client: Option<Client>,
    /// Why the server can't be reached, when it can't.
    pub(crate) offline: Option<String>,
    pub(crate) page: Page,
    /// Each list page's selection, by page.
    pub(crate) sel: [usize; 6],
    pub(crate) detail: Option<Detail>,
    pub(crate) flow_tab: FlowTab,
    /// The Runs list's selection, and how far Status and History are scrolled.
    pub(crate) flow_sel: usize,
    pub(crate) flow_scroll: usize,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) tab: usize,
    /// The editor has the keyboard: keys type.
    pub(crate) typing: bool,
    pub(crate) modal: Option<Modal>,
    /// The `:` prompt's text, while open.
    pub(crate) prompt: Option<String>,
    pub(crate) note: Option<Note>,
    pub(crate) quit: bool,
    pub(crate) refreshed: Instant,
    /// The text pane's height when last drawn, for paging.
    pub(crate) text_height: usize,
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
            sel: [0; 6],
            detail: None,
            flow_tab: FlowTab::Status,
            flow_sel: 0,
            flow_scroll: 0,
            tabs: vec![],
            tab: 0,
            typing: false,
            modal: None,
            prompt: None,
            note: None,
            quit: false,
            refreshed: Instant::now(),
            text_height: 20,
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
                Some(Ok(d)) => self.detail = Some(d),
                Some(Err(_)) => self.detail = None,
                None => {}
            }
        }
    }

    /// What the clock does between keys: read the server again now and then, and check an
    /// edited tab once typing settles.
    pub(crate) fn tick(&mut self) {
        if self.refreshed.elapsed() >= REFRESH && self.modal.is_none() {
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

    fn show_tab(&mut self, i: usize) {
        self.tab = i;
        self.page = Page::Editor;
    }

    /// Open a deployment's or saved flow's text in a tab: the saved one when it is newer than
    /// what is deployed (someone saved it for a person to review), else the deployed one.
    pub(crate) fn edit(&mut self, name: &str) {
        if let Some(i) = self.tabs.iter().position(|t| t.path.is_none() && t.slug.as_deref() == Some(name)) {
            return self.show_tab(i);
        }
        let Some(c) = self.client.as_ref() else {
            return self.fail("no server to open it from");
        };
        let deployed_at = c.deployment(name).and_then(|d| d["deployed_at_epoch_seconds"].as_u64());
        let saved_at = c.saved(name).and_then(|f| f["saved_at_epoch_seconds"].as_u64());
        let text = match (deployed_at, saved_at) {
            (Some(d), Some(s)) if s > d => c.saved_text(name),
            (Some(_), _) => c.get(&format!("/deployments/{}/document", crate::mcp::seg(name))).and_then(|d| agent::text_of(name, &d)),
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
                    self.flow_sel = 0;
                    self.flow_scroll = 0;
                    self.flow_tab = FlowTab::Status;
                }
                self.detail = Some(d);
                self.page = Page::Flow;
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
            Page::Flow => match self.flow_tab {
                FlowTab::Runs => self.detail.as_ref().map_or(0, |d| d.runs.len()),
                _ => 0,
            },
            Page::Editor => 0,
        }
    }

    // ── doing ──────────────────────────────────────────────────────────────────────────────

    /// Save a tab: to its file, else on the server as a saved flow.
    pub(crate) fn save(&mut self, i: usize, to: Option<PathBuf>) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        if let Some(p) = to {
            tab.name = p.file_name().and_then(|s| s.to_str()).unwrap_or("flow").to_string();
            tab.path = Some(p);
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

    /// Stage a tab's deploy, and show what it asks a person to approve.
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
        match c.stage(&plan, &slug) {
            Ok(staged) => self.modal = Some(Modal::Approve { slug, plan: Box::new(plan), staged, text, tab: i }),
            Err(e) => self.fail(e),
        }
    }

    /// Deploy what the dialog shows, approving it as us.
    fn approve(&mut self) {
        let Some(Modal::Approve { slug, plan, staged, text, tab }) = self.modal.take() else { return };
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

    fn lifecycle(&mut self, slug: &str, action: &str) {
        let Some(c) = self.client.as_ref() else { return };
        match c.lifecycle(slug, action) {
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

    fn ask_close(&mut self, i: usize) {
        match self.tabs.get(i) {
            Some(t) if t.dirty() => {
                self.modal = Some(Modal::Confirm { title: format!("Close {}?", t.name), lines: vec!["It has changes that aren't saved: closing it drops them.".into()], then: Then::Close(i) })
            }
            Some(_) => self.close_tab(i),
            None => {}
        }
    }

    fn ask_quit(&mut self) {
        let edited: Vec<String> = self.tabs.iter().filter(|t| t.dirty()).map(|t| t.name.clone()).collect();
        if edited.is_empty() {
            self.quit = true;
        } else {
            self.modal = Some(Modal::Confirm { title: "Quit?".into(), lines: vec![format!("Not saved: {}.", edited.join(", ")), "Quitting drops those changes.".into()], then: Then::Quit });
        }
    }

    fn format(&mut self, i: usize) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        match editing::format(&tab.buf.text()) {
            Ok(t) => {
                let (row, col) = (tab.buf.row, tab.buf.col);
                tab.buf = Buffer::new(&t);
                tab.buf.go(row as u32 + 1, col as u32 + 1);
                tab.edited = Some(Instant::now() - SETTLE);
            }
            Err(e) => self.fail(format!("can't format it: {e}")),
        }
    }

    /// Turn a tab's "View runs" on or off.
    fn toggle_runs(&mut self, i: usize) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        if tab.runs.take().is_some() {
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

    fn run_command(&mut self, line: &str) {
        let line = line.trim();
        let (cmd, arg) = line.split_once(' ').map(|(c, a)| (c, a.trim())).unwrap_or((line, ""));
        let tab = (!self.tabs.is_empty()).then_some(self.tab);
        match (cmd, arg) {
            ("q" | "quit", _) => self.ask_quit(),
            ("q!", _) => self.quit = true,
            ("e" | "open" | "edit", a) if !a.is_empty() => self.open_arg(a),
            ("new", "") => self.page = Page::Library,
            ("new", a) => self.open_sample(a),
            ("w" | "save", a) => match tab {
                Some(i) => self.save(i, (!a.is_empty()).then(|| PathBuf::from(a))),
                None => self.fail("no tab to save"),
            },
            ("deploy", _) => match tab {
                Some(i) => self.deploy(i),
                None => self.fail("no tab to deploy: open a flow first"),
            },
            ("flow", a) if !a.is_empty() => self.open_flow(a),
            (action @ ("run" | "pause" | "stop"), a) if !a.is_empty() => self.lifecycle(a, action),
            ("connect" | "server", a) => {
                if !a.is_empty() {
                    self.server_arg = Some(a.to_string());
                }
                self.connect();
                if self.offline.is_none() {
                    self.say(format!("connected to {}", self.client.as_ref().map(Client::server).unwrap_or_default()));
                }
            }
            ("r" | "refresh", _) => self.refresh(),
            ("help" | "h", _) => self.modal = Some(Modal::Help { scroll: 0 }),
            _ => self.fail(format!(":{line}? (:help lists the commands)")),
        }
    }

    // ── keys ───────────────────────────────────────────────────────────────────────────────

    pub(crate) fn paste(&mut self, text: &str) {
        if let Some(p) = self.prompt.as_mut() {
            p.push_str(text.lines().next().unwrap_or(""));
        } else if self.page == Page::Editor && self.typing && let Some(tab) = self.tabs.get_mut(self.tab) {
            tab.buf.insert_str(text);
            tab.edited = Some(Instant::now());
        }
    }

    pub(crate) fn key(&mut self, k: KeyEvent) {
        // AltGr arrives as ctrl+alt: a character typed, not a shortcut.
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && k.code == KeyCode::Char('c') {
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
        if self.page == Page::Editor && !self.tabs.is_empty() {
            if ctrl && self.editor_shortcut(k.code) {
                return;
            }
            if self.typing && self.tabs[self.tab].runs.is_none() {
                return self.type_key(k, ctrl);
            }
        }
        self.page_key(k);
    }

    /// The editor's ctrl keys, typing or not.
    fn editor_shortcut(&mut self, code: KeyCode) -> bool {
        let i = self.tab;
        match code {
            KeyCode::Char('s') => self.save(i, None),
            KeyCode::Char('d') => self.deploy(i),
            KeyCode::Char('r') => self.toggle_runs(i),
            KeyCode::Char('f') => self.format(i),
            KeyCode::Char('w') => self.ask_close(i),
            KeyCode::PageDown | KeyCode::Right => self.tab = (i + 1) % self.tabs.len(),
            KeyCode::PageUp | KeyCode::Left => self.tab = (i + self.tabs.len() - 1) % self.tabs.len(),
            KeyCode::Char('q') => self.ask_quit(),
            _ => return false,
        }
        true
    }

    fn type_key(&mut self, k: KeyEvent, ctrl: bool) {
        let height = self.text_height;
        let tab = &mut self.tabs[self.tab];
        let b = &mut tab.buf;
        let before = b.lines.clone();
        match k.code {
            KeyCode::Esc => self.typing = false,
            KeyCode::Char(c) if !ctrl => b.insert(c),
            KeyCode::Enter => b.newline(true),
            KeyCode::Tab => b.tab(),
            KeyCode::Backspace => b.backspace(),
            KeyCode::Delete => b.delete(),
            KeyCode::Left => b.left(),
            KeyCode::Right => b.right(),
            KeyCode::Up => b.down(-1),
            KeyCode::Down => b.down(1),
            KeyCode::Home if ctrl => b.go(1, 1),
            KeyCode::End if ctrl => b.go(u32::MAX, u32::MAX),
            KeyCode::Home => b.home(),
            KeyCode::End => b.end(),
            KeyCode::PageUp => b.down(-(height as isize)),
            KeyCode::PageDown => b.down(height as isize),
            _ => {}
        }
        if b.lines != before {
            tab.edited = Some(Instant::now());
        }
    }

    fn select(&mut self, page: Page, by: isize) {
        let n = self.rows(page);
        if page == Page::Flow {
            if self.flow_tab == FlowTab::Runs {
                self.flow_sel = (self.flow_sel as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
            } else {
                self.flow_scroll = (self.flow_scroll as isize + by).max(0) as usize;
            }
            return;
        }
        let s = &mut self.sel[index(page)];
        *s = (*s as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
    }

    fn page_key(&mut self, k: KeyEvent) {
        let page = self.page;
        match k.code {
            KeyCode::Char(c @ '1'..='6') => {
                let to = PAGES[c as usize - '1' as usize].0;
                if to == Page::Flow && self.detail.is_none() {
                    return self.fail("no flow open: pick one on the flows page");
                }
                self.page = to;
                if to == Page::Editor && !self.tabs.is_empty() && self.tabs[self.tab].runs.is_none() {
                    self.typing = true;
                }
                return;
            }
            KeyCode::Char(':') => return self.prompt = Some(String::new()),
            KeyCode::Char('?') => return self.modal = Some(Modal::Help { scroll: 0 }),
            KeyCode::Char('q') => return self.ask_quit(),
            KeyCode::Char('r') if page != Page::Editor => {
                if self.client.is_none() {
                    self.connect();
                } else {
                    self.refresh();
                }
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => return self.select(page, -1),
            KeyCode::Down | KeyCode::Char('j') => return self.select(page, 1),
            KeyCode::PageUp => return self.select(page, -10),
            KeyCode::PageDown => return self.select(page, 10),
            _ => {}
        }
        let sel = self.sel[index(page)];
        match page {
            Page::Overview => {
                let slug = self.client.as_ref().and_then(|c| c.deployments.get(sel)).and_then(|d| d["slug"].as_str()).map(str::to_string);
                match (k.code, slug) {
                    (KeyCode::Enter, Some(s)) => self.open_flow(&s),
                    (KeyCode::Char('e'), Some(s)) => self.edit(&s),
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
                    _ => {}
                }
            }
            Page::Flow => self.flow_key(k),
            Page::Editor => self.editor_key(k),
            Page::Library => {
                if k.code == KeyCode::Enter
                    && let Some(s) = samples::samples().get(sel)
                {
                    self.open_sample(s.name);
                }
            }
        }
    }

    /// Do what a deployment waits on a person for: start it, or review and deploy its text.
    fn resolve(&mut self, slug: &str, wait: &str) {
        if wait.starts_with("to start it") {
            self.modal = Some(Modal::Confirm { title: format!("Run {slug}?"), lines: vec![format!("\"{slug}\" is {}: run it again, as you.", self.status_of(slug))], then: Then::Lifecycle(slug.to_string(), "run") });
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

    fn status_of(&self, slug: &str) -> String {
        self.client.as_ref().and_then(|c| c.deployment(slug)).and_then(|d| d["status"].as_str()).unwrap_or("?").to_string()
    }

    fn flow_key(&mut self, k: KeyEvent) {
        let Some(slug) = self.detail.as_ref().map(|d| d.slug.clone()) else { return };
        match k.code {
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.flow_tab = match self.flow_tab {
                    FlowTab::Status => FlowTab::Runs,
                    FlowTab::Runs => FlowTab::History,
                    FlowTab::History => FlowTab::Status,
                };
                self.flow_scroll = 0;
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.flow_tab = match self.flow_tab {
                    FlowTab::Status => FlowTab::History,
                    FlowTab::Runs => FlowTab::Status,
                    FlowTab::History => FlowTab::Runs,
                };
                self.flow_scroll = 0;
            }
            KeyCode::Char('s') => self.flow_tab = FlowTab::Status,
            KeyCode::Char('u') => self.flow_tab = FlowTab::Runs,
            KeyCode::Char('t') => self.flow_tab = FlowTab::History,
            KeyCode::Esc | KeyCode::Backspace => self.page = Page::Flows,
            KeyCode::Char('e') => self.edit(&slug),
            KeyCode::Char('g') => self.lifecycle(&slug, "run"),
            KeyCode::Char('p') => self.lifecycle(&slug, "pause"),
            KeyCode::Char('x') => self.modal = Some(Modal::Confirm { title: format!("Stop {slug}?"), lines: vec![format!("\"{slug}\" stops answering and running until it is run again.")], then: Then::Lifecycle(slug, "stop") }),
            KeyCode::Enter if self.flow_tab == FlowTab::Runs => self.explain(),
            _ => {}
        }
    }

    /// Show the selected run over its version's text.
    fn explain(&mut self) {
        let (Some(c), Some(d)) = (self.client.as_ref(), self.detail.as_ref()) else { return };
        // Listed newest first.
        let Some(entry) = d.runs.iter().rev().nth(self.flow_sel) else { return };
        let title = agent::run_line(entry);
        match agent::explain(&d.slug, &c.get(&document_path(&d.slug, entry)).unwrap_or(Value::Null), entry) {
            Ok(text) => self.modal = Some(Modal::Explain { title, lines: text.lines().map(str::to_string).collect(), scroll: 0 }),
            Err(e) => self.fail(e),
        }
    }

    fn editor_key(&mut self, k: KeyEvent) {
        let i = self.tab;
        if let Some(runs) = self.tabs.get_mut(i).and_then(|t| t.runs.as_mut()) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('v') => {
                    self.tabs[i].runs = None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    runs.sel = runs.sel.saturating_sub(1);
                    self.reload_run(i);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    runs.sel = (runs.sel + 1).min(runs.entries.len().saturating_sub(1));
                    self.reload_run(i);
                }
                KeyCode::Char('n') | KeyCode::Enter => {
                    let n = runs.noted().len();
                    if n > 0 {
                        runs.step = (runs.step + 1) % n;
                    }
                }
                KeyCode::Char('N') => {
                    let n = runs.noted().len();
                    if n > 0 {
                        runs.step = (runs.step + n - 1) % n;
                    }
                }
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Enter | KeyCode::Char('i') => self.typing = true,
            KeyCode::Char('[') => self.tab = (i + self.tabs.len() - 1) % self.tabs.len(),
            KeyCode::Char(']') => self.tab = (i + 1) % self.tabs.len(),
            KeyCode::Char('w') => self.ask_close(i),
            KeyCode::Char('s') => self.save(i, None),
            KeyCode::Char('d') => self.deploy(i),
            KeyCode::Char('v') => self.toggle_runs(i),
            KeyCode::Char('f') => self.format(i),
            _ => {}
        }
    }

    fn reload_run(&mut self, i: usize) {
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
        match modal {
            Modal::Approve { .. } => match k.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('a') => self.approve(),
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.modal = None;
                    self.say("not deployed");
                }
                _ => {}
            },
            Modal::Explain { scroll, lines, .. } => match k.code {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll = (*scroll + 1).min(lines.len().saturating_sub(1)),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll = (*scroll + 10).min(lines.len().saturating_sub(1)),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.modal = None,
                _ => {}
            },
            Modal::Help { scroll } => match k.code {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                _ => self.modal = None,
            },
            Modal::Confirm { .. } => match k.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    let Some(Modal::Confirm { then, .. }) = self.modal.take() else { return };
                    match then {
                        Then::Quit => self.quit = true,
                        Then::Close(i) => self.close_tab(i),
                        Then::Lifecycle(slug, action) => self.lifecycle(&slug, action),
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => self.modal = None,
                _ => {}
            },
        }
    }
}

fn document_path(slug: &str, entry: &Value) -> String {
    match entry.get("unit").and_then(Value::as_str).filter(|u| !u.is_empty()) {
        Some(u) => format!("/deployments/{}/document?unit={u}", crate::mcp::seg(slug)),
        None => format!("/deployments/{}/document", crate::mcp::seg(slug)),
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
