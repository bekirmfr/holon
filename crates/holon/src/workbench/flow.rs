//! A deployment's page: its dashboard, then its runs, history, state and access, each a list on
//! the left and what the picked row holds on the right, in tabs. From here a person goes back to
//! a point (its options, then a dry run, then the restore), calls it, and changes its access.

use holon_author::agent;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value};

use super::app::{App, Ask, List, Modal, Page, Said, Then, Tone};
use super::client::{Client, Detail, Moment, Point};
use super::graph::GraphView;
use super::review::{self, Review};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Section {
    #[default]
    Status,
    Runs,
    History,
    State,
    Access,
    Graph,
}

pub(crate) const SECTIONS: [(Section, &str); 6] =
    [(Section::Status, "Status"), (Section::Runs, "Runs"), (Section::History, "History"), (Section::State, "State"), (Section::Access, "Access"), (Section::Graph, "Graph")];

/// The right pane's tabs, by section.
pub(crate) fn tabs(s: Section) -> &'static [&'static str] {
    match s {
        Section::Status => &[],
        Section::Runs => &["Detail", "Steps", "Timeline", "Error"],
        Section::History => &["Detail", "Code", "State"],
        Section::State => &["Value", "History"],
        Section::Access => &["Detail", "History"],
        Section::Graph => &["Statement", "Newest run"],
    }
}

/// A line of the right pane.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    Text(Tone, String),
    /// A name and its value, the names lined up.
    Field(String, String, Tone),
    /// A step on a timeline: from and to, of `total`.
    Bar { label: String, from: f64, to: f64, total: f64, note: String },
    /// A line of a run's text, and what its statements gave there.
    Code { n: u32, text: String, notes: Vec<String> },
}

fn text(tone: Tone, s: impl Into<String>) -> Row {
    Row::Text(tone, s.into())
}

fn field(k: &str, v: impl Into<String>) -> Row {
    Row::Field(k.to_string(), v.into(), Tone::Plain)
}

/// A state cell, as the state section lists it.
#[derive(Clone, Debug)]
pub(crate) struct Cell {
    pub(crate) name: String,
    pub(crate) value: Value,
    pub(crate) collection: bool,
}

/// A row of the access section.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Access {
    /// Its grant, whole.
    Grant,
    /// Something it may do: granted, revoked, or asked for and not granted.
    Item { item: String, how: Held },
    /// Its secrets, whole.
    Secrets,
    Secret { name: String, set: bool },
}

/// What an env secret's variable starts with (secret_sources.rs).
pub(crate) const ENV: &str = "HOLON_SECRET_";

/// Setting a secret: where its value comes from, as the web editor's deploy settings ask it.
pub(crate) struct SecretChoice {
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) set: bool,
    /// Where it comes from now (`local`, `env:…`, `vault:…`), when set.
    pub(crate) current: Option<String>,
    /// The `HOLON_SECRET_…` names the server reads, and its `--dotenv` file.
    pub(crate) env: Vec<String>,
    pub(crate) dotenv: Option<String>,
    pub(crate) sel: usize,
}

/// One of the places a secret's value can come from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    /// Typed here, sealed on the server.
    Value,
    /// One of the names the server reads.
    Env(String),
    /// A name typed, the server's list not having it (yet).
    OtherEnv,
    Vault,
}

impl SecretChoice {
    pub(crate) fn choices(&self) -> Vec<Source> {
        let mut out = vec![Source::Value];
        out.extend(self.env.iter().cloned().map(Source::Env));
        out.extend([Source::OtherEnv, Source::Vault]);
        out
    }

    /// The variable its name suggests: `HOLON_SECRET_API_TOKEN` for `api_token`.
    pub(crate) fn suggested(&self) -> String {
        format!("{ENV}{}", self.name.to_uppercase())
    }

    /// Picked first: where it comes from now; else the variable its name suggests, when the
    /// server reads one; else a value.
    pub(crate) fn first(&self) -> usize {
        let choices = self.choices();
        let at = |s: &Source| choices.iter().position(|c| c == s);
        let now = match self.current.as_deref() {
            Some(c) if c.starts_with("vault:") => at(&Source::Vault),
            Some(c) => c.strip_prefix("env:").and_then(|v| at(&Source::Env(v.to_string()))),
            None => None,
        };
        now.or_else(|| at(&Source::Env(self.suggested()))).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    Granted,
    Revoked,
    NotGranted,
}

#[derive(Default)]
pub(crate) struct FlowView {
    pub(crate) section: Section,
    /// The row picked, by section.
    pub(crate) sel: [usize; 6],
    /// The right pane's tab, by section.
    pub(crate) tab: [usize; 6],
    /// The right pane has the arrows: they scroll it.
    pub(crate) right: bool,
    pub(crate) scroll: usize,
    /// What the right pane shows, and for which row and tab.
    pub(crate) pane: Option<(String, Vec<Row>)>,
    pub(crate) cells: Vec<Cell>,
    /// The graph of the text deployed, once its section has been opened.
    pub(crate) graph: Option<GraphView>,
    /// The graph shows every node, not only the statements (as it does unless a says otherwise).
    pub(crate) every: bool,
}

impl App {
    /// The flow page's graph: every node a row, or the statements with their nodes under
    /// each. The same statement stays picked, as its first node or as the statement.
    pub(crate) fn flow_every(&mut self) {
        let line = self.flow.graph.as_ref().and_then(|g| g.row(self.flow.sel[Section::Graph as usize])).map(|r| r.line);
        self.flow.every = !self.flow.every;
        self.flow_load();
        if let (Some(line), Some(g)) = (line, self.flow.graph.as_ref()) {
            self.flow.sel[Section::Graph as usize] = g.row_at(line);
            self.flow_load();
        }
        self.say(if self.flow.every { "every node, each a row: a again for the statements" } else { "the statements, their nodes under each: a again for every node" });
    }
}

impl FlowView {
    pub(crate) fn sel(&self) -> usize {
        self.sel[self.section as usize]
    }

    pub(crate) fn tab(&self) -> usize {
        self.tab[self.section as usize]
    }
}

/// Microseconds as a person reads a time of day.
pub(crate) fn at_ms(v: &Value) -> String {
    v.as_u64().map(|ms| crate::engines::time_text(ms as i64 * 1000).replace('T', " ")).unwrap_or_default()
}

pub(crate) fn took(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

pub(crate) fn failed(r: &Value) -> bool {
    r["error"].is_string() || r["status"].as_u64().is_some_and(|s| s >= 500)
}

/// What came in: its method and URL, else what started it.
pub(crate) fn came_in(r: &Value) -> String {
    match (r["method"].as_str(), r["url"].as_str()) {
        (Some(m), Some(u)) => format!("{m} {u}"),
        _ => match (r["kind"].as_str(), r["route"].as_str()) {
            (Some("tick"), Some(label)) => format!("tick, {label}"),
            (Some(k), _) => k.to_string(),
            _ => "run".into(),
        },
    }
}

fn short(s: &str, n: usize) -> &str {
    &s[..s.len().min(n)]
}

/// The access section's rows: the grant and what it covers, then the secrets.
pub(crate) fn access_rows(d: &Detail) -> Vec<Access> {
    let g = &d.grant["grant"];
    let mut rows = vec![Access::Grant];
    let words = |v: &Value| -> Vec<String> { v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
    rows.extend(words(&g["items"]).into_iter().map(|item| Access::Item { item, how: Held::Granted }));
    rows.extend(words(&g["revoked"]).into_iter().map(|item| Access::Item { item, how: Held::Revoked }));
    rows.extend(words(&d.manifest["not_granted"]).into_iter().map(|item| Access::Item { item, how: Held::NotGranted }));
    rows.push(Access::Secrets);
    let set: Vec<String> = d.secrets["secrets"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_string)).collect();
    let mut names: Vec<String> = set.clone();
    names.extend(words(&d.secrets["names"]));
    names.extend(words(&d.secrets["reads"]));
    names.sort();
    names.dedup();
    rows.extend(names.into_iter().map(|name| Access::Secret { set: set.contains(&name), name }));
    rows
}

impl App {
    pub(crate) fn flow_rows(&self) -> usize {
        let Some(d) = &self.detail else { return 0 };
        match self.flow.section {
            Section::Status => d.versions.len(),
            Section::Runs => d.runs.len(),
            Section::History => d.moments().len(),
            Section::State => self.flow.cells.len(),
            Section::Access => access_rows(d).len(),
            Section::Graph => self.flow.graph.as_ref().map_or(0, |g| g.rows.len()),
        }
    }

    /// After the deployment is read again: its state's cells too, and the right pane anew.
    pub(crate) fn flow_refreshed(&mut self) {
        if let (Some(c), Some(d)) = (self.client.as_ref(), self.detail.as_ref()) {
            self.flow.cells = match c.state(&d.slug) {
                Ok(Value::Object(m)) => m
                    .into_iter()
                    .map(|(k, value)| match k.strip_prefix("#collection:") {
                        Some(name) => Cell { name: name.to_string(), value, collection: true },
                        None => Cell { name: k, value, collection: false },
                    })
                    .collect(),
                _ => vec![],
            };
        }
        let n = self.flow_rows();
        let s = self.flow.section as usize;
        self.flow.sel[s] = self.flow.sel[s].min(n.saturating_sub(1));
        let scroll = self.flow.scroll;
        let key = self.flow.pane.take().map(|(k, _)| k);
        self.flow_load();
        // The same row and tab, read again: where it was scrolled to stays.
        if self.flow.pane.as_ref().map(|(k, _)| k) == key.as_ref() {
            self.flow.scroll = scroll;
        }
    }

    /// Read what the right pane shows for the row and tab picked, unless it shows it already.
    pub(crate) fn flow_load(&mut self) {
        let (Some(c), Some(d)) = (self.client.as_ref(), self.detail.as_ref()) else { return };
        let (s, sel, tab) = (self.flow.section, self.flow.sel(), self.flow.tab());
        let key: String;
        let make: Box<dyn Fn() -> Vec<Row>>;
        match s {
            Section::Status => {
                self.flow.pane = None;
                return;
            }
            Section::Runs => {
                let Some(r) = d.runs.iter().rev().nth(sel) else {
                    self.flow.pane = None;
                    return;
                };
                key = format!("run {} {tab}", r["seq"]);
                make = Box::new(move || run_pane(c, d, r, tab));
            }
            Section::History => {
                let Some(m) = d.moments().get(sel).copied() else {
                    self.flow.pane = None;
                    return;
                };
                key = format!("{m:?} {tab} {}", d.entry["unit"]);
                make = Box::new(move || history_pane(c, d, m, tab));
            }
            Section::State => {
                let Some(cell) = self.flow.cells.get(sel).cloned() else {
                    self.flow.pane = None;
                    return;
                };
                key = format!("cell {} {tab} {}", cell.name, cell.value);
                make = Box::new(move || state_pane(c, d, &cell, tab));
            }
            Section::Access => {
                let Some(row) = access_rows(d).get(sel).cloned() else {
                    self.flow.pane = None;
                    return;
                };
                key = format!("{row:?} {tab} {}", d.grant["hash"]);
                make = Box::new(move || access_pane(c, d, &row, tab));
            }
            Section::Graph => {
                // Built again for a new version, a new run (what it reached), or every node.
                let built = format!("{} {} {}", d.entry["unit"], self.flow.every, d.runs.len());
                if self.flow.graph.as_ref().map(|g| &g.key) != Some(&built) {
                    let mut g = match c.text(&d.slug, None) {
                        Ok(text) => GraphView::build(built, &text, self.flow.every),
                        Err(e) => {
                            let mut g = GraphView::build(built, "", false);
                            g.error = Some(e);
                            g
                        }
                    };
                    g.reach(c, d);
                    self.flow.sel[Section::Graph as usize] = sel.min(g.rows.len().saturating_sub(1));
                    self.flow.graph = Some(g);
                }
                let sel = self.flow.sel();
                let Some(g) = self.flow.graph.as_ref() else { return };
                key = format!("graph {sel} {tab} {}", g.key);
                make = Box::new(move || g.pane(sel, tab, Some((c, d))));
            }
        }
        if self.flow.pane.as_ref().map(|(k, _)| k) != Some(&key) {
            let rows = make();
            self.flow.pane = Some((key, rows));
            self.flow.scroll = 0;
        }
    }

    pub(crate) fn flow_section(&mut self, s: Section) {
        self.flow.section = s;
        self.flow.right = false;
        self.flow_load();
    }

    pub(crate) fn flow_detail_tab(&mut self, t: usize) {
        let n = tabs(self.flow.section).len();
        if n > 0 {
            self.flow.tab[self.flow.section as usize] = t % n;
            self.flow_load();
        }
    }

    /// A row clicked, or the right pane: it has the keys now.
    pub(crate) fn flow_pick(&mut self, list: List, i: usize) {
        if list == List::Detail {
            self.flow.right = true;
            return;
        }
        let s = self.flow.section as usize;
        self.flow.sel[s] = i.min(self.flow_rows().saturating_sub(1));
        self.flow.right = false;
        self.flow_load();
    }

    pub(crate) fn flow_scroll(&mut self, list: List, by: isize) {
        if list == List::Detail {
            // Kept in range as it is drawn, where its wrapped lines are counted.
            self.flow.scroll = (self.flow.scroll as isize + by).max(0) as usize;
        } else {
            self.flow_move(by);
        }
    }

    fn flow_move(&mut self, by: isize) {
        let n = self.flow_rows();
        let s = self.flow.section as usize;
        self.flow.sel[s] = (self.flow.sel[s] as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        self.flow_load();
    }

    pub(crate) fn flow_key(&mut self, k: KeyEvent) {
        let Some(slug) = self.detail.as_ref().map(|d| d.slug.clone()) else { return };
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let at = SECTIONS.iter().position(|(s, _)| *s == self.flow.section).unwrap_or(0);
        match k.code {
            // The dashboard has no panes: ← → are the sections there; elsewhere tab and shift+tab.
            KeyCode::Left | KeyCode::Char('h') if tabs(self.flow.section).is_empty() => return self.flow_section(SECTIONS[(at + SECTIONS.len() - 1) % SECTIONS.len()].0),
            KeyCode::Left | KeyCode::Char('h') if !self.flow.right => return,
            KeyCode::Right | KeyCode::Char('l') if !self.flow.right && !tabs(self.flow.section).is_empty() && self.flow_rows() > 0 => {
                self.flow.right = true;
                return;
            }
            KeyCode::Right | KeyCode::Char('l') if tabs(self.flow.section).is_empty() => return self.flow_section(SECTIONS[(at + 1) % SECTIONS.len()].0),
            // In the right pane, ← → go through its tabs; ← from the first goes back to the list.
            KeyCode::Left | KeyCode::Char('h') if self.flow.tab() == 0 => {
                self.flow.right = false;
                return;
            }
            KeyCode::Left | KeyCode::Char('h') => return self.flow_detail_tab(self.flow.tab() - 1),
            KeyCode::Right | KeyCode::Char('l') => return self.flow_detail_tab((self.flow.tab() + 1).min(tabs(self.flow.section).len() - 1)),
            KeyCode::Tab => return self.flow_section(SECTIONS[(at + 1) % SECTIONS.len()].0),
            KeyCode::BackTab => return self.flow_section(SECTIONS[(at + SECTIONS.len() - 1) % SECTIONS.len()].0),
            KeyCode::Char('t') => return self.flow_detail_tab(self.flow.tab() + 1),
            KeyCode::Char('T') => return self.flow_detail_tab(self.flow.tab() + tabs(self.flow.section).len().max(1) - 1),
            KeyCode::Esc if self.flow.right => {
                self.flow.right = false;
                return;
            }
            KeyCode::Esc => return self.go(Page::Flows),
            KeyCode::Char('e') => return self.edit(&slug),
            KeyCode::Char('c') => return self.call(&slug),
            KeyCode::Char('g') => return self.ask_lifecycle(&slug, "run"),
            KeyCode::Char('p') => return self.ask_lifecycle(&slug, "pause"),
            KeyCode::Char('x') => return self.ask_lifecycle(&slug, "stop"),
            KeyCode::Char('D') | KeyCode::Delete if self.flow.section != Section::Access => {
                self.modal = Some(Modal::Confirm {
                    title: format!("Remove {slug}?"),
                    lines: vec!["It stops, and its port is freed. Its runs, history and state stay on record; deploying it again brings it back.".into()],
                    then: Then::Remove(slug),
                });
                return;
            }
            _ => {}
        }
        if self.flow.right {
            let to = |by: isize, at: usize| (at as isize + by).max(0) as usize;
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.flow.scroll = to(-1, self.flow.scroll),
                KeyCode::Down | KeyCode::Char('j') => self.flow.scroll = to(1, self.flow.scroll),
                KeyCode::PageUp => self.flow.scroll = to(-10, self.flow.scroll),
                KeyCode::PageDown => self.flow.scroll = to(10, self.flow.scroll),
                KeyCode::Home => self.flow.scroll = 0,
                KeyCode::End => self.flow.scroll = usize::MAX / 2,
                _ => self.flow_enter(k, &slug),
            }
            return;
        }
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.flow_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.flow_move(1),
            KeyCode::PageUp => self.flow_move(-10),
            KeyCode::PageDown => self.flow_move(10),
            KeyCode::Home => self.flow_move(isize::MIN / 2),
            KeyCode::End => self.flow_move(isize::MAX / 2),
            KeyCode::Char('J') if shift => self.flow_scroll(List::Detail, 1),
            KeyCode::Char('K') if shift => self.flow_scroll(List::Detail, -1),
            _ => self.flow_enter(k, &slug),
        }
    }

    /// Enter, and the keys that act on the row picked.
    fn flow_enter(&mut self, k: KeyEvent, slug: &str) {
        let Some(d) = self.detail.as_ref() else { return };
        let sel = self.flow.sel();
        match (self.flow.section, k.code) {
            (Section::Status, KeyCode::Enter | KeyCode::Char('b')) => {
                let Some(n) = d.versions.get(sel).and_then(|v| v["number"].as_u64()) else { return };
                match d.version_point(n) {
                    Some(p) => self.ask_restore(slug, p),
                    None => self.fail(format!("no version {n}")),
                }
            }
            (Section::Runs, KeyCode::Enter) => {
                if let Some(seq) = d.runs.iter().rev().nth(sel).and_then(|r| r["seq"].as_u64()) {
                    self.step_through(slug, seq);
                }
            }
            (Section::Runs, KeyCode::Char('b')) => {
                let Some(i) = d.runs.len().checked_sub(sel + 1) else { return };
                if let Some(p) = d.point(Moment::Run(i)) {
                    self.ask_restore(slug, p);
                }
            }
            (Section::History, KeyCode::Enter | KeyCode::Char('b')) => {
                let Some(m) = d.moments().get(sel).copied() else { return };
                match d.point(m) {
                    Some(p) => self.ask_restore(slug, p),
                    None => self.fail("there is nothing here to go back to: an event that names no version"),
                }
            }
            (Section::State, KeyCode::Enter) => self.flow_detail_tab(1),
            (Section::Graph, KeyCode::Enter) => {
                if let Some(line) = self.flow.graph.as_ref().and_then(|g| g.row(sel)).map(|r| r.line) {
                    self.edit_at(slug, line);
                }
            }
            (Section::Graph, KeyCode::Char('a')) => self.flow_every(),
            (Section::Access, KeyCode::Enter) => match access_rows(d).get(sel).cloned() {
                Some(Access::Item { item, how: Held::Granted }) => {
                    self.modal = Some(Modal::Confirm {
                        title: format!("Revoke from {slug}?"),
                        lines: vec![format!("It may no longer {}.", agent::describe(&item)), "A run that needs it fails, or it pauses, as its grant says. Granting it back undoes this.".into()],
                        then: Then::Grant { slug: slug.to_string(), item, back: false },
                    })
                }
                Some(Access::Item { item, how: Held::Revoked | Held::NotGranted }) => {
                    self.modal = Some(Modal::Confirm {
                        title: format!("Grant to {slug}?"),
                        lines: vec![format!("It may {} again, approved by you.", agent::describe(&item))],
                        then: Then::Grant { slug: slug.to_string(), item, back: true },
                    })
                }
                Some(Access::Secret { name, set }) => {
                    let current = d.secrets["secrets"].as_array().into_iter().flatten().find(|s| s["name"] == *name).and_then(|s| s["source"].as_str()).map(str::to_string);
                    let (env, dotenv) = self.client.as_ref().map(|c| c.env_names()).unwrap_or_default();
                    let mut choice = SecretChoice { slug: slug.to_string(), name, set, current, env, dotenv, sel: 0 };
                    choice.sel = choice.first();
                    self.modal = Some(Modal::SecretSource(choice));
                }
                _ => {}
            },
            (Section::Access, KeyCode::Delete | KeyCode::Char('-')) => {
                if let Some(Access::Secret { name, set: true }) = access_rows(d).get(sel).cloned() {
                    self.modal = Some(Modal::Confirm {
                        title: format!("Remove {name}?"),
                        lines: vec![format!("{slug} reads nothing as {name} until it is set again. Its versions stay on record.")],
                        then: Then::RemoveSecret(slug.to_string(), name),
                    });
                }
            }
            _ => {}
        }
    }

    fn ask_lifecycle(&mut self, slug: &str, action: &'static str) {
        let (title, line) = match action {
            "run" => (format!("Run {slug}?"), "It takes calls and runs its clocks again, as you.".to_string()),
            "pause" => (format!("Pause {slug}?"), "Calls get 503 until it runs again; runs in flight finish. Its state stays.".to_string()),
            _ => (format!("Stop {slug}?"), "It stops and frees its port. Running it again starts a new session.".to_string()),
        };
        self.modal = Some(Modal::Confirm { title, lines: vec![format!("\"{slug}\" is {} now.", self.status_of(slug)), line], then: Then::Lifecycle(slug.to_string(), action, None) });
    }

    // ── going back ─────────────────────────────────────────────────────────────────────────

    /// The options going back to a point has: both, its code, its state.
    pub(crate) fn ask_restore(&mut self, slug: &str, point: Point) {
        let sel = restore_options(&point).iter().position(|o| o.3.is_ok()).unwrap_or(0);
        self.modal = Some(Modal::Restore { slug: slug.to_string(), point, sel });
    }

    pub(crate) fn restore_key(&mut self, k: KeyEvent) {
        let Some(Modal::Restore { slug, point, sel }) = self.modal.as_mut() else { return };
        let options = restore_options(point);
        match k.code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(options.len() - 1),
            KeyCode::Enter => {
                let (_, code, state, ok) = &options[*sel];
                if let Err(why) = ok {
                    let why = why.clone();
                    return self.fail(why);
                }
                let (slug, point, code, state) = (slug.clone(), point.clone(), *code, *state);
                self.dry_run(&slug, point, code, state);
            }
            _ => {}
        }
    }

    /// Ask the server what the restore would do, and show it, for a person to confirm.
    fn dry_run(&mut self, slug: &str, point: Point, code: bool, state: bool) {
        let Some(c) = self.client.as_ref() else { return };
        let preview = match c.restore(slug, &point.at, code, state, None) {
            Ok(p) => p,
            Err(e) => return self.fail(e),
        };
        let mut review = Review::default();
        let refused = preview["refused"].as_str().map(str::to_string);
        if let Some(why) = &refused {
            review.top.push((Tone::Bad, format!("The code of {} can't be deployed on this server now: {why}", point.name)));
        }
        let deploys = preview["code"].as_u64();
        if code && deploys.is_none() && refused.is_none() {
            review.top.push((Tone::Dim, format!("{} ran on the code deployed now: only its state is put back.", point.name)));
        }
        let needs: Vec<String> = preview["needs_approval"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
        if let Some(n) = deploys {
            if needs.is_empty() {
                review.top.push((Tone::Good, format!("Version {n}'s code deploys again, with the grant {slug} has now.")));
            } else {
                let k = needs.len();
                review.top.push((Tone::Warn, format!("Version {n}'s code needs {k} permission{} today's grant doesn't cover, marked +: confirming approves {}, as you.", if k == 1 { "" } else { "s" }, if k == 1 { "it" } else { "them" })));
            }
            match c.code_texts(slug, n) {
                Ok((now, then, note)) => {
                    match review::code(format!("Its code: deployed now → version {n}'s"), &now, &then, &preview["touches"]) {
                        Some(hunk) => review.hunks.push(hunk),
                        None => review.top.push((Tone::Dim, format!("Version {n}'s text is the text deployed now."))),
                    }
                    if !note.is_empty() {
                        review.top.push((Tone::Dim, note.trim().to_string()));
                    }
                }
                Err(e) => review.top.push((Tone::Bad, format!("Its text can't be shown: {e}"))),
            }
        }
        review.hunks.extend(review::touched(&preview));
        if !preview["state"].is_null() {
            review.hunks.push(review::state(&preview["state"]));
        }
        self.modal = Some(Modal::DryRun { slug: slug.to_string(), point, code, state, review, needs, refused: refused.is_some() });
    }

    pub(crate) fn dry_run_key(&mut self, k: KeyEvent) {
        let Some(Modal::DryRun { review, refused, .. }) = self.modal.as_mut() else { return };
        match k.code {
            _ if review.key(k) => {}
            KeyCode::Esc | KeyCode::Char('x') => {
                self.modal = None;
                self.say("nothing changed");
            }
            KeyCode::Enter | KeyCode::Char('y') if !*refused => self.restore(),
            KeyCode::Enter => self.modal = None,
            _ => {}
        }
    }

    /// The restore the dry run showed, done.
    fn restore(&mut self) {
        let Some(Modal::DryRun { slug, point, code, state, needs, .. }) = self.modal.take() else { return };
        let Some(c) = self.client.as_ref() else { return };
        match c.restore(&slug, &point.at, code, state, Some(&needs)) {
            Ok(done) => {
                let mut said = vec![];
                if let Some(n) = done["code"].as_u64() {
                    let unit = done["unit"].as_str().unwrap_or_default();
                    said.push(format!("version {n}'s code is deployed again, as version {} ({})", done["version"], short(unit, 12)));
                }
                if let Some(session) = done["state"]["session"].as_u64() {
                    said.push(format!("the state {} left is live again, in session {session}", point.name));
                }
                if said.is_empty() {
                    said.push("nothing to change".into());
                }
                self.refresh();
                self.say(format!("{slug}: {}", said.join("; ")));
            }
            Err(e) => self.fail(e),
        }
    }

    // ── doing ──────────────────────────────────────────────────────────────────────────────

    pub(crate) fn remove(&mut self, slug: &str) {
        let Some(c) = self.client.as_ref() else { return };
        match c.remove(slug) {
            Ok(()) => {
                if self.detail.as_ref().is_some_and(|d| d.slug == slug) {
                    self.detail = None;
                    if self.page == Page::Flow {
                        self.page = Page::Flows;
                    }
                }
                self.refresh();
                self.say(format!("removed \"{slug}\": its runs and history stay on record"));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(crate) fn change_grant(&mut self, slug: &str, item: &str, back: bool) {
        let Some(c) = self.client.as_ref() else { return };
        let one = [item.to_string()];
        let (revoke, grant): (&[String], &[String]) = if back { (&[], &one) } else { (&one, &[]) };
        match c.change_grant(slug, revoke, grant) {
            Ok(()) => {
                self.refresh();
                self.say(format!("{slug} {} {}", if back { "may again" } else { "may no longer" }, agent::describe(item)));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(crate) fn remove_secret(&mut self, slug: &str, name: &str) {
        let Some(c) = self.client.as_ref() else { return };
        match c.remove_secret(slug, name) {
            Ok(()) => {
                self.refresh();
                self.say(format!("removed {name}: {slug} reads nothing as it until it is set"));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(crate) fn answer_input(&mut self) {
        let Some(Modal::Input { value, then, .. }) = self.modal.take() else { return };
        let (slug, name, from) = match then {
            Ask::Admin(a) => return self.admin_answer(a, value),
            Ask::SaveAs(i) => return self.save_as(i, &value),
            Ask::Secret(slug, name) => (slug, name, None),
            Ask::SecretFrom(slug, name, kind) => (slug, name, Some(kind)),
        };
        if value.trim().is_empty() {
            return self.fail("not set: nothing was typed");
        }
        match from {
            None => self.set_secret(&slug, &name, Err(&value)),
            Some(kind) => self.set_secret(&slug, &name, Ok(&format!("{kind}:{}", value.trim()))),
        }
    }

    /// Set a secret: a value typed (`Err`, never shown again), or where it is read from (`Ok`).
    fn set_secret(&mut self, slug: &str, name: &str, how: Result<&str, &str>) {
        let Some(c) = self.client.as_ref() else { return };
        let set = match how {
            Ok(from) => c.set_secret_from(slug, name, from),
            Err(value) => c.set_secret(slug, name, value),
        };
        match set {
            Ok(v) => {
                self.modal = None;
                self.refresh();
                let version = v["version"].as_u64().map(|n| format!(" (v{n})")).unwrap_or_default();
                let from = how.map(|f| format!(", read from {f}")).unwrap_or_default();
                self.say(format!("set {name}{version} for {slug}{from}"));
            }
            Err(e) => self.fail(e),
        }
    }

    /// Where a secret comes from, picked: a value to type, a name the server reads (set at once),
    /// another name or a Vault field to type.
    pub(crate) fn secret_source_key(&mut self, k: KeyEvent) {
        let Some(Modal::SecretSource(c)) = self.modal.as_mut() else { return };
        let choices = c.choices();
        match k.code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Up | KeyCode::Char('k') => c.sel = c.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => c.sel = (c.sel + 1).min(choices.len() - 1),
            KeyCode::Enter => {
                let (slug, name, set) = (c.slug.clone(), c.name.clone(), c.set);
                let ask = |title: String, lines: Vec<String>, value: &str, masked: bool, then: Ask| Modal::Input { title, lines, value: value.to_string(), masked, then };
                self.modal = Some(match &choices[c.sel] {
                    Source::Env(var) => {
                        let from = format!("env:{var}");
                        return self.set_secret(&slug, &name, Ok(&from));
                    }
                    Source::Value => ask(
                        format!("{} {name}", if set { "Change" } else { "Set" }),
                        vec![format!("The value {slug} reads as {name}. It is sealed on the server, never shown again, here or anywhere."), "Typed as dots. Enter sets it, esc leaves it as it is.".into()],
                        "",
                        true,
                        Ask::Secret(slug, name),
                    ),
                    Source::OtherEnv => ask(
                        format!("{name} from the server's environment"),
                        vec![format!("A variable of the server's environment or its --dotenv file, starting {ENV}. Its name only: the value stays where it is, read again every minute."), "Enter sets it, esc leaves it as it is.".into()],
                        ENV,
                        false,
                        Ask::SecretFrom(slug, name, "env"),
                    ),
                    Source::Vault => ask(
                        format!("{name} from Vault"),
                        vec!["A field of a HashiCorp Vault KV v2 secret, as <mount>/<path>#<field> (secret/stripe#api_key), read with the server's VAULT_ADDR and VAULT_TOKEN, again every minute.".into(), "Enter sets it, esc leaves it as it is.".into()],
                        "",
                        false,
                        Ask::SecretFrom(slug, name, "vault"),
                    ),
                });
            }
            _ => {}
        }
    }

    // ── calling ────────────────────────────────────────────────────────────────────────────

    /// The call form: its routes, actions and clocks, one to send.
    pub(crate) fn call(&mut self, slug: &str) {
        let Some(c) = self.client.as_ref() else { return self.fail("no server") };
        let Some(entry) = c.deployment(slug) else { return self.fail(format!("\"{slug}\" isn't deployed")) };
        let text = c.text(slug, None).unwrap_or_default();
        let mut items: Vec<Callable> = vec![];
        if let Ok(plan) = agent::plan(&text) {
            for r in plan.routes {
                if let Some((method, path)) = r.split_once(' ') {
                    items.push(Callable::Route { method: method.to_string(), path: path.to_string() });
                }
            }
        }
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("action ") {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
                if !name.is_empty() && !items.iter().any(|i| matches!(i, Callable::Action(n) if *n == name)) {
                    items.push(Callable::Action(name));
                }
            }
        }
        for t in super::client::strings(entry, "triggers") {
            if !(t == "Ingress" || t == "Clock" || t.starts_with("Http")) {
                items.push(Callable::Clock(t));
            }
        }
        if items.is_empty() {
            return self.fail(format!("{slug} has no route, action or clock to call"));
        }
        let mut form = CallForm { slug: slug.to_string(), items, sel: 0, field: 0, url: String::new(), body: String::new(), answer: vec![] };
        form.pick(0);
        self.modal = Some(Modal::Call(form));
    }

    pub(crate) fn call_key(&mut self, k: KeyEvent) {
        let Some(Modal::Call(form)) = self.modal.as_mut() else { return };
        let fields = form.fields();
        match k.code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Tab => form.field = (form.field + 1) % (fields + 1),
            KeyCode::BackTab => form.field = (form.field + fields) % (fields + 1),
            KeyCode::Up if form.field == 0 => form.pick(form.sel.saturating_sub(1)),
            KeyCode::Down if form.field == 0 => form.pick(form.sel + 1),
            KeyCode::Up => form.field -= 1,
            KeyCode::Down => form.field = (form.field + 1).min(fields),
            KeyCode::Enter => self.send_call(),
            KeyCode::Backspace => {
                if let Some(s) = form.focused() {
                    s.pop();
                }
            }
            KeyCode::Char(ch) => {
                if form.field == 0 && fields > 0 {
                    form.field = 1;
                }
                if let Some(s) = form.focused() {
                    s.push(ch);
                }
            }
            _ => {}
        }
    }

    fn send_call(&mut self) {
        let Some(Modal::Call(form)) = self.modal.as_ref() else { return };
        let Some(c) = self.client.as_ref() else { return };
        let slug = form.slug.clone();
        let answer: Vec<Said> = match &form.items[form.sel] {
            Callable::Route { method, .. } => match c.request(&slug, method, &form.url, &form.body) {
                Ok(v) => answer_lines(&v),
                Err(e) => vec![(Tone::Bad, e)],
            },
            Callable::Action(name) => match c.action(&slug, name, &form.body) {
                // What `holon action` says on stderr is about the run (its number), unless it failed.
                Ok((lines, went_wrong)) => lines.into_iter().map(|(err, l)| (if went_wrong { Tone::Bad } else if err { Tone::Dim } else { Tone::Plain }, l)).collect(),
                Err(e) => vec![(Tone::Bad, e)],
            },
            Callable::Clock(label) => match c.tick(&slug, label) {
                Ok(lines) => lines.into_iter().map(|l| (Tone::Plain, l)).collect(),
                Err(e) => vec![(Tone::Bad, e)],
            },
        };
        self.refresh();
        if let Some(Modal::Call(form)) = self.modal.as_mut() {
            form.answer = answer;
        }
    }
}

/// The ways back from a point: (what, its code, its state, whether it can).
pub(crate) fn restore_options(p: &Point) -> Vec<(String, bool, bool, Result<(), String>)> {
    vec![
        ("Its code and its state".into(), true, true, p.code.clone().and(p.state.clone())),
        ("Its code only: deploy that version again".into(), true, false, p.code.clone()),
        ("Its state only: put back the state it left".into(), false, true, p.state.clone()),
    ]
}

/// A diff as `code_diff` writes it, coloured by line.
pub(crate) fn diff_lines(diff: &str) -> Vec<Said> {
    diff.lines()
        .map(|l| {
            let tone = if l.starts_with("+ ") {
                Tone::Added
            } else if l.starts_with("- ") {
                Tone::Removed
            } else if l.starts_with("Its text") || l.starts_with("Version") {
                Tone::Dim
            } else {
                Tone::Plain
            };
            (tone, l.to_string())
        })
        .collect()
}

/// An HTTP answer: its status, then its body, laid out when it is JSON.
fn answer_lines(v: &Value) -> Vec<Said> {
    let status = v["status"].as_u64().unwrap_or_default();
    let tone = if status >= 400 { Tone::Bad } else { Tone::Good };
    let mut lines = vec![(tone, format!("answered {status}"))];
    let body = v["body"].as_str().unwrap_or_default();
    match serde_json::from_str::<Value>(body).ok().and_then(|j| serde_json::to_string_pretty(&j).ok()) {
        Some(pretty) => lines.extend(pretty.lines().map(|l| (Tone::Plain, l.to_string()))),
        None => lines.extend(body.lines().map(|l| (Tone::Plain, l.to_string()))),
    }
    lines
}

/// What a flow can be sent: a route, an action, or a clock to fire now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Callable {
    Route { method: String, path: String },
    Action(String),
    Clock(String),
}

impl Callable {
    pub(crate) fn label(&self) -> String {
        match self {
            Callable::Route { method, path } => format!("{method} {path}"),
            Callable::Action(name) => format!("action {name}"),
            Callable::Clock(label) => format!("clock {label}"),
        }
    }
}

pub(crate) struct CallForm {
    pub(crate) slug: String,
    pub(crate) items: Vec<Callable>,
    pub(crate) sel: usize,
    /// 0 the list, then the fields: a route's URL and body, an action's input.
    pub(crate) field: usize,
    pub(crate) url: String,
    pub(crate) body: String,
    pub(crate) answer: Vec<Said>,
}

impl CallForm {
    /// How many fields the one picked has.
    pub(crate) fn fields(&self) -> usize {
        match &self.items[self.sel] {
            Callable::Route { method, .. } if method == "GET" || method == "DELETE" => 1,
            Callable::Route { .. } => 2,
            Callable::Action(_) => 1,
            Callable::Clock(_) => 0,
        }
    }

    /// The field's label, by number (from 1).
    pub(crate) fn field_name(&self, i: usize) -> &'static str {
        match (&self.items[self.sel], i) {
            (Callable::Route { .. }, 1) => "URL",
            (Callable::Route { .. }, _) => "Body",
            (Callable::Action(_), _) => "Input (JSON)",
            (Callable::Clock(_), _) => "",
        }
    }

    pub(crate) fn field_value(&self, i: usize) -> &str {
        match (&self.items[self.sel], i) {
            (Callable::Route { .. }, 1) => &self.url,
            _ => &self.body,
        }
    }

    fn focused(&mut self) -> Option<&mut String> {
        match (&self.items[self.sel], self.field) {
            (_, 0) | (Callable::Clock(_), _) => None,
            (Callable::Route { .. }, 1) => Some(&mut self.url),
            _ => Some(&mut self.body),
        }
    }

    /// Pick one: its fields filled as a first call would have them.
    pub(crate) fn pick(&mut self, i: usize) {
        self.sel = i.min(self.items.len() - 1);
        self.answer.clear();
        match &self.items[self.sel] {
            Callable::Route { method, path } => {
                self.url = path.clone();
                self.body = if method == "GET" || method == "DELETE" { String::new() } else { "{}".into() };
            }
            Callable::Action(_) => self.body = "{}".into(),
            Callable::Clock(_) => self.body.clear(),
        }
    }

    pub(crate) fn paste(&mut self, text: &str) {
        if self.field == 0 && self.fields() > 0 {
            self.field = 1;
        }
        if let Some(s) = self.focused() {
            s.push_str(&text.replace(['\r', '\n'], " "));
        }
    }
}

// ── the right pane ───────────────────────────────────────────────────────────────────────────

fn run_pane(c: &Client, d: &Detail, r: &Value, tab: usize) -> Vec<Row> {
    match tab {
        0 => run_detail(d, r),
        1 => match c.run_view(&d.slug, r) {
            Ok(v) => {
                let mut rows = vec![text(Tone::Dim, v.head.clone())];
                if v.no_steps {
                    rows.push(text(Tone::Dim, "This run recorded no steps: the values are left out."));
                }
                rows.extend(v.text.iter().enumerate().map(|(i, l)| {
                    let n = i as u32 + 1;
                    Row::Code { n, text: l.clone(), notes: v.notes.get(&n).cloned().unwrap_or_default() }
                }));
                rows.extend(v.after.iter().map(|l| text(if l.starts_with("failed") { Tone::Bad } else { Tone::Plain }, l.clone())));
                rows.push(text(Tone::Dim, "enter: step through it in the editor"));
                rows
            }
            Err(e) => vec![text(Tone::Bad, e)],
        },
        2 => match c.run_view(&d.slug, r) {
            Ok(v) if !v.steps.is_empty() => {
                let total = v.steps.iter().map(|s| s.end_us).fold(0.0, f64::max).max(r["duration_ms"].as_f64().unwrap_or(0.0) * 1000.0).max(1.0);
                let mut rows = vec![text(Tone::Dim, format!("0 to {}, each statement as it ran", took(total / 1000.0)))];
                rows.extend(v.steps.iter().map(|s| Row::Bar {
                    label: v.text.get(s.line as usize - 1).map(|l| l.trim().to_string()).unwrap_or_default(),
                    from: s.start_us,
                    to: s.end_us,
                    total,
                    note: took((s.end_us - s.start_us) / 1000.0),
                }));
                rows
            }
            Ok(_) => vec![text(Tone::Dim, "This run recorded no steps.")],
            Err(e) => vec![text(Tone::Bad, e)],
        },
        _ => {
            let mut rows = vec![];
            if let Some(e) = r["error"].as_str() {
                rows.push(text(Tone::Head, "It failed"));
                rows.extend(e.lines().map(|l| text(Tone::Bad, l)));
            }
            if let Some(s) = r["status"].as_u64().filter(|s| *s >= 400) {
                rows.push(text(Tone::Head, format!("It answered {s}")));
                rows.extend(r["response_body"].as_str().unwrap_or_default().lines().take(40).map(|l| text(Tone::Plain, l)));
            }
            if rows.is_empty() {
                rows.push(text(Tone::Good, "It didn't fail."));
            }
            rows
        }
    }
}

fn run_detail(d: &Detail, r: &Value) -> Vec<Row> {
    let mut rows = vec![];
    let seq = r["seq"].as_u64().unwrap_or_default();
    rows.push(Row::Field("Run".into(), format!("#{seq}"), if failed(r) { Tone::Bad } else { Tone::Plain }));
    rows.push(field("At", at_ms(&r["at_ms"])));
    rows.push(field("Took", took(r["duration_ms"].as_f64().unwrap_or(0.0))));
    rows.push(field("Came in", came_in(r)));
    if let Some(route) = r["route"].as_str().filter(|_| r["kind"] == "request") {
        rows.push(field("Route", route));
    }
    if let Some(s) = r["status"].as_u64() {
        rows.push(Row::Field("Answered".into(), s.to_string(), if s >= 400 { Tone::Bad } else { Tone::Good }));
    }
    let unit = r["unit"].as_str().unwrap_or_default();
    let version = d.ran_on(r).map(|n| format!("v{n}, ")).unwrap_or_default();
    rows.push(field("Version", format!("{version}{}", short(unit, 12))));
    if let Some(s) = r["session"].as_u64() {
        rows.push(field("Session", s.to_string()));
    }
    if let Some(n) = r["retries"].as_u64().filter(|n| *n > 0) {
        rows.push(field("Retries", n.to_string()));
    }
    if r["parked"].as_array().is_some_and(|p| !p.is_empty()) {
        rows.push(Row::Field("Waits at".into(), super::client::strings(r, "parked").join(", "), Tone::Warn));
    }
    if let Some(id) = r["run_id"].as_str().filter(|s| !s.is_empty()) {
        rows.push(field("Run id", short(id, 16)));
    }
    if let Some(g) = r["grant"].as_str().filter(|s| !s.is_empty()) {
        rows.push(field("Grant", short(g, 12)));
    }
    if let Some(e) = r["error"].as_str() {
        rows.push(Row::Field("Failed".into(), e.lines().next().unwrap_or_default().to_string(), Tone::Bad));
    }
    if !r["request"].is_null() {
        rows.push(text(Tone::Head, "What came in"));
        rows.push(text(Tone::Plain, agent::show(&r["request"])));
    }
    if let Some(b) = r["response_body"].as_str().filter(|b| !b.is_empty()) {
        rows.push(text(Tone::Head, "Its answer"));
        rows.extend(b.lines().take(40).map(|l| text(Tone::Plain, l)));
    }
    let performed: Vec<&Value> = r["performed"].as_array().into_iter().flatten().collect();
    if !performed.is_empty() {
        rows.push(text(Tone::Head, "What it did outside"));
        rows.extend(performed.iter().map(|p| text(Tone::Plain, format!("{}  {}", p["capability"].as_str().unwrap_or("?"), agent::show(&p["payload"])))));
    }
    let calls: Vec<&Value> = r["rounds"].as_array().into_iter().flatten().filter_map(|x| x["exchanges"].as_array()).flatten().collect();
    if !calls.is_empty() {
        rows.push(text(Tone::Head, "What it called"));
        rows.extend(calls.iter().map(|x| text(Tone::Plain, format!("{} → {}", agent::show(&x["request"]), agent::show(&x["response"])))));
    }
    rows.push(text(Tone::Dim, "enter: step through it in the editor · b: go back to it"));
    rows
}

fn going_back(p: &Point) -> Vec<Row> {
    let say = |what: &str, r: &Result<(), String>| match r {
        Ok(()) => Row::Field(what.into(), "can be put back".into(), Tone::Good),
        Err(why) => Row::Field(what.into(), why.clone(), Tone::Dim),
    };
    vec![text(Tone::Head, format!("Going back to {}", p.name)), say("Its code", &p.code), say("Its state", &p.state), text(Tone::Dim, "enter: the options, then a dry run, then the restore")]
}

fn history_pane(c: &Client, d: &Detail, m: Moment, tab: usize) -> Vec<Row> {
    let point = d.point(m);
    let version = match m {
        Moment::Run(i) => d.ran_on(&d.runs[i]),
        Moment::Event(i) => d.events[i]["version"].as_u64(),
    };
    match tab {
        0 => {
            let mut rows = match m {
                Moment::Run(i) => run_detail(d, &d.runs[i]).into_iter().filter(|r| matches!(r, Row::Field(..))).collect(),
                Moment::Event(i) => {
                    let e = &d.events[i];
                    let mut rows = vec![
                        Row::Field("What".into(), e["what"].as_str().unwrap_or_default().to_string(), Tone::Plain),
                        field("Kind", e["kind"].as_str().unwrap_or_default()),
                        field("At", at_ms(&e["at_ms"])),
                        field("By", e["by"].as_str().unwrap_or("the host")),
                        field("After run", format!("#{}", e["run_seq"])),
                    ];
                    if let Some(v) = version.and_then(|n| d.version(n)) {
                        rows.push(field("Version", format!("v{}, {}", v["number"], short(v["unit"].as_str().unwrap_or_default(), 12))));
                        if let Some(by) = v["by"].as_str() {
                            rows.push(field("Deployed by", by));
                        }
                        if let Some(from) = v["restored_from"].as_u64() {
                            rows.push(field("Restores", format!("v{from}")));
                        }
                    }
                    rows
                }
            };
            if let Some(p) = &point {
                rows.extend(going_back(p));
            }
            rows
        }
        1 => match version {
            Some(n) if d.version(n).is_some_and(|v| v["current"] == true) => vec![text(Tone::Dim, format!("Version {n} is the version deployed now: its text is the text now."))],
            Some(n) => match c.code_diff(&d.slug, n) {
                Ok(diff) => diff_lines(&diff).into_iter().map(|(t, l)| text(t, l)).collect(),
                Err(e) => vec![text(Tone::Bad, e)],
            },
            None => vec![text(Tone::Dim, "It names no version of the code.")],
        },
        _ => match m {
            Moment::Run(i) => {
                let r = &d.runs[i];
                let mut rows = vec![text(Tone::Head, "What it read, as the run began")];
                rows.extend(crate::opscmd::state_lines(&d.slug, &r["state_before"]).into_iter().map(|l| text(Tone::Plain, l)));
                rows.push(text(Tone::Dim, if r["state_after"].is_string() { "What it left is kept: going back to it (enter) puts it back." } else { "What it left wasn't kept." }));
                rows
            }
            Moment::Event(_) => vec![
                text(Tone::Dim, "The state a version left is kept when the next one is deployed."),
                text(Tone::Dim, "Going back to it (enter) asks for a dry run first, which says what it changes."),
            ],
        },
    }
}

fn state_pane(c: &Client, d: &Detail, cell: &Cell, tab: usize) -> Vec<Row> {
    if tab == 1 {
        return match c.changes(&d.slug, &cell.name, None) {
            Ok(lines) => lines.into_iter().enumerate().map(|(i, l)| text(if i == 0 { Tone::Head } else { Tone::Plain }, l)).collect(),
            Err(e) => vec![text(Tone::Bad, e)],
        };
    }
    if cell.collection {
        return match c.collection(&d.slug, &cell.name) {
            Ok((items, more)) => {
                let mut rows = vec![text(Tone::Dim, format!("the collection {}, {} item{} shown", cell.name, items.len(), if items.len() == 1 { "" } else { "s" }))];
                rows.extend(items.into_iter().map(|(k, v)| Row::Field(k, agent::show(&v), Tone::Plain)));
                if more {
                    rows.push(text(Tone::Dim, format!("and more: holon state {} --collection {}", d.slug, cell.name)));
                }
                rows
            }
            Err(e) => vec![text(Tone::Bad, e)],
        };
    }
    let shown = agent::show(&cell.value);
    let mut rows = vec![text(Tone::Dim, format!("state.{}", cell.name))];
    rows.extend(shown.lines().map(|l| text(Tone::Plain, l)));
    rows.push(text(Tone::Dim, "enter: the runs that changed it"));
    rows
}

fn access_pane(c: &Client, d: &Detail, row: &Access, tab: usize) -> Vec<Row> {
    let changes = || -> Vec<Row> {
        let all: Vec<&Value> = d.grant["grant"]["changes"].as_array().into_iter().flatten().collect();
        if all.is_empty() {
            return vec![text(Tone::Dim, "Its grant hasn't changed since it was deployed.")];
        }
        all.iter().rev().map(|ch| Row::Field(at_ms(&json!(ch["at"].as_u64().map(|s| s * 1000))), ch["change"].as_str().unwrap_or_default().to_string(), Tone::Plain)).collect()
    };
    match (row, tab) {
        (Access::Grant, 0) => {
            if d.grant.is_null() {
                return vec![text(Tone::Dim, "Its grant can't be read here: only an admin reads it.")];
            }
            crate::grantcmd::grant_lines(&d.slug, &d.grant, &d.manifest).into_iter().map(|l| text(Tone::Plain, l)).collect()
        }
        (Access::Item { item, how }, 0) => {
            let (status, tone, act) = match how {
                Held::Granted => ("granted", Tone::Good, "enter: revoke it"),
                Held::Revoked => ("revoked", Tone::Warn, "enter: grant it back"),
                Held::NotGranted => ("asked for, not granted", Tone::Bad, "enter: grant it, as you"),
            };
            vec![field("It may", agent::describe(item)), field("As", item.clone()), Row::Field("Now".into(), status.into(), tone), text(Tone::Dim, act)]
        }
        (Access::Secrets, 0) => crate::secretcmd::list_lines(&d.secrets).into_iter().map(|l| text(Tone::Plain, l)).collect(),
        (Access::Secret { name, set }, 0) => {
            let s = d.secrets["secrets"].as_array().into_iter().flatten().find(|s| s["name"] == *name);
            let mut rows = vec![field("Secret", name.clone())];
            match s {
                Some(s) => {
                    rows.push(Row::Field("Set".into(), format!("v{}, from {}, {}", s["version"], s["source"].as_str().unwrap_or("?"), at_ms(&json!(s["set_at"].as_u64().map(|t| t * 1000)))), Tone::Good));
                    if let Some(p) = s["problem"].as_str() {
                        rows.push(Row::Field("Problem".into(), p.to_string(), Tone::Bad));
                    }
                }
                None => rows.push(Row::Field("Set".into(), "not set: a run that reads it gets nothing".into(), Tone::Warn)),
            }
            let reads = d.secrets["reads"].as_array().is_some_and(|r| r.iter().any(|n| n == name));
            rows.push(field("Read", if reads { "by the version deployed now" } else { "not by the version deployed now" }));
            rows.push(text(Tone::Dim, if *set { "enter: change it (a value, a name the server reads, or Vault) · del: remove it" } else { "enter: set it (a value, a name the server reads, or Vault)" }));
            rows
        }
        (Access::Secret { name, .. }, _) => match c.secret_history(&d.slug, name) {
            Ok(lines) => lines.into_iter().map(|l| text(Tone::Plain, l)).collect(),
            Err(e) => vec![text(Tone::Dim, e)],
        },
        _ => changes(),
    }
}
