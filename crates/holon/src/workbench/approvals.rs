//! The Approvals page's parts beside its deploys: the runs waiting at an approval (`await
//! approval(…)`), each a person decides here as the web console's Approvals › Runs waiting does
//! (`POST /deployments/<slug>/runs/<run_id>/decide`, durable.rs); and the access changes waiting for
//! people (`GET /access/proposals`, access_proposals.rs), approved, declined or withdrawn here as
//! the web console's Approvals › Access changes does.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::{json, Value};

use holon_author::agent;

use super::app::{App, Ask, Modal};
use crate::mcp::seg;

/// The Approvals page's parts, as its tabs: versions to deploy, runs waiting at an approval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Part {
    #[default]
    Deploys,
    Runs,
    Access,
}

pub(crate) const PARTS: [(Part, &str); 3] = [(Part::Deploys, "Deploys"), (Part::Runs, "Runs waiting"), (Part::Access, "Access changes")];

impl Part {
    /// The tab after this one, or before it, round.
    pub(crate) fn next(self, back: bool) -> Part {
        let at = PARTS.iter().position(|(p, _)| *p == self).unwrap_or(0);
        PARTS[(at + if back { PARTS.len() - 1 } else { 1 }) % PARTS.len()].0
    }
}

/// What deploying asks again, before it deploys, when its runs would wait at approvals no one here
/// can decide (the web's `askAgain`, touches.ts; the access plan's D3): the question's lines,
/// naming each with why; None when none would. A row is one the server marks not ready
/// (`ready`, deploy.rs `approvals_unready`) that every run waits at: one only the runs someone
/// starts wait at is marked in the review, and not asked about again. The server deploys it either
/// way: someone may be bound after.
pub(crate) fn ask_again(name: &str, touches: &Value) -> Option<Vec<String>> {
    let rows: Vec<&Value> = touches
        .as_array()?
        .iter()
        .filter(|t| t["kind"] == "approval" && t["change"] != "gone" && !t["ready"].is_null() && t["waits"] == "every run")
        .collect();
    if rows.is_empty() {
        return None;
    }
    let mut lines = vec![format!("Runs of \"{name}\" would wait at {} no one here can decide yet:", if rows.len() == 1 { "an approval" } else { "approvals" })];
    lines.extend(rows.iter().map(|t| format!("• {}: {}", t["name"].as_str().unwrap_or("?"), t["ready"].as_str().unwrap_or("not ready"))));
    lines.push("They wait there until someone here may take each step (Admin › People, or holon bind, binds them).".into());
    Some(lines)
}

/// The most of an access change's why its question says, in characters (the web's `WHY_CHARS`), so
/// it can't push what the change gives out of sight (R42).
const WHY_CHARS: usize = 300;

/// Why access change `p` isn't `me`'s to approve, as the web says it (access.ts `notYours`): its
/// author, someone who approved it, or someone who could act as whom it gives more. None when it is.
pub(crate) fn not_yours(p: &Value, me: &str) -> Option<String> {
    if p["author"].as_str() == Some(me) {
        return Some("you proposed it: people other than you approve it".into());
    }
    if p["approvals"].as_array().into_iter().flatten().any(|a| a["who"].as_str() == Some(me)) {
        return Some("you approved it".into());
    }
    let way = p["ways_in"].as_array().into_iter().flatten().filter_map(Value::as_str).find(|w| w.starts_with(&format!("{me} ")))?;
    Some(format!("it gives more to someone you could act as ({way}): it is approved by someone who couldn't"))
}

/// What an access change gives, a clause: "admin on the host to dee; …".
pub(crate) fn gives(p: &Value) -> String {
    p["gives"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")
}

/// `s` cut to `n` characters, with … when cut.
fn clipped(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// A decision asked for: the run, its point and step, and whether it approves. A decline says why;
/// an approval may.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decision {
    pub(crate) slug: String,
    pub(crate) run_id: String,
    pub(crate) name: String,
    pub(crate) slot: u64,
    pub(crate) approve: bool,
}

/// A proposed version to decline: its id, flow and version, and who proposed it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Proposed {
    pub(crate) id: u64,
    pub(crate) slug: String,
    pub(crate) version: u64,
    pub(crate) by: String,
    pub(crate) agent: bool,
}

impl Proposed {
    pub(crate) fn of(p: &Value) -> Proposed {
        Proposed {
            id: p["id"].as_u64().unwrap_or(0),
            slug: p["slug"].as_str().unwrap_or("?").to_string(),
            version: p["version"].as_u64().unwrap_or(0),
            by: p["by"].as_str().unwrap_or("someone").to_string(),
            agent: p["agent"] == true,
        }
    }
}

/// What a proposal deploys (`GET /proposals/<id>`, with its document): its text, as the document
/// prints it (`agent::text_of`), compiled. Refused when that builds another unit than the one
/// proposed (R79): a person approves what was proposed, or nothing. The server checks no person's
/// deploy against a proposal; it marks one approved only when the units match.
pub(crate) fn proposed_plan(p: &Value) -> Result<(agent::Plan, String), String> {
    let slug = p["slug"].as_str().ok_or("the proposal names no flow")?;
    let document = p.get("document").filter(|d| !d.is_null()).ok_or("the server sent the proposal without its document")?;
    let text = agent::text_of(slug, document)?;
    let plan = agent::plan(&text)?;
    match p["unit"].as_str() {
        Some(unit) if unit == plan.unit => Ok((plan, text)),
        Some(unit) => Err(format!(
            "its text builds {} where {} was proposed: this isn't what was proposed. Decline it (d), and say so.",
            &plan.unit[..12.min(plan.unit.len())],
            &unit[..12.min(unit.len())]
        )),
        None => Err("the proposal names no unit".into()),
    }
}

/// A proposal in a line, as Approvals lists it (the wait's words): its version, who proposed it
/// and why.
pub(crate) fn proposal_wait(p: &Value) -> String {
    let by = p["by"].as_str().unwrap_or("someone");
    let who = if p["agent"] == true { format!("{by}, an agent") } else { by.to_string() };
    let why = p["why"].as_str().filter(|w| !w.trim().is_empty()).unwrap_or("no why was given");
    format!("to review v{} proposed by {who}, at {}: {why}", p["version"].as_u64().unwrap_or(0), crate::mcp::when(&json!(p["at_ms"].as_u64().unwrap_or(0) / 1000)))
}

/// The first approval point of a run that the caller may take a step of now, and that step: its
/// `yours`, as the server answers it for the caller.
pub(crate) fn yours(run: &Value) -> Option<(&Value, u64)> {
    run["waiting"].as_array()?.iter().find_map(|w| Some((w, w["yours"].as_array()?.first()?.as_u64()?)))
}

/// An approval point's steps in a line, as the web draws its chain: ✓ a step filled, ◻ one open,
/// · one not open yet; each by its role, with how many of how many when it takes more than one.
pub(crate) fn chain(w: &Value) -> String {
    let Some(slots) = w["slots"].as_array() else { return "an event".into() };
    slots
        .iter()
        .map(|s| {
            let by = s["by"].as_array().map_or(0, Vec::len) as u64;
            let people = s["people"].as_u64().unwrap_or(1);
            let mark = if by >= people { "✓" } else if s["open"] == true { "◻" } else { "·" };
            let name = s["role"].as_str().map(String::from).unwrap_or_else(|| if s["kind"] == "agent" { "an agent".into() } else { "a person".into() });
            if people > 1 { format!("{mark} {name} {by}/{people}") } else { format!("{mark} {name}") }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// What a waiting run is, in a few words: its route or kind, and who started it.
pub(crate) fn what(run: &Value) -> String {
    let what = run["route"].as_str().or(run["kind"].as_str()).unwrap_or("?");
    match run["by"].as_str() {
        Some(by) => format!("{what}, started by {by}"),
        None => what.to_string(),
    }
}

/// The step of `w` numbered `slot`, by its role: " as approver", or nothing for a lone step.
fn as_role(w: &Value, slot: u64) -> String {
    w["slots"].as_array().and_then(|s| s.get(slot as usize)).and_then(|s| s["role"].as_str()).map(|r| format!(" as {r}")).unwrap_or_default()
}

/// What the point shows those who decide it, a line each: what its flow named, or the run's input.
fn shown(w: &Value) -> Vec<String> {
    let lines = |v: &Value| -> Vec<String> {
        match v {
            Value::Object(m) => m.iter().map(|(k, v)| format!("  {k}: {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect(),
            Value::Null => vec![],
            v => vec![format!("  {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))],
        }
    };
    if !w["shows"].is_null() {
        let mut out = vec!["It shows:".to_string()];
        out.extend(lines(&w["shows"]));
        out
    } else if !w["input"].is_null() {
        let mut out = vec!["Its run was started with:".to_string()];
        out.extend(lines(&w["input"]));
        out
    } else {
        vec![]
    }
}

/// What deciding leaves the approval waiting for, after this step: the steps not filled but this
/// one, or none.
fn then(w: &Value, slot: u64) -> String {
    let rest: Vec<String> = w["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["slot"].as_u64() != Some(slot) && (s["by"].as_array().map_or(0, Vec::len) as u64) < s["people"].as_u64().unwrap_or(1))
        .map(|s| s["role"].as_str().map(String::from).unwrap_or_else(|| format!("slot {}", s["slot"])))
        .collect();
    let mine = w["slots"].as_array().and_then(|s| s.get(slot as usize));
    let more_here = mine.map_or(0, |s| s["people"].as_u64().unwrap_or(1).saturating_sub(s["by"].as_array().map_or(0, Vec::len) as u64 + 1));
    match (rest.is_empty(), more_here) {
        (true, 0) => "The run goes on once you do.".into(),
        (true, n) => format!("It then waits for {n} more {}.", if n == 1 { "person" } else { "people" }),
        (false, _) => format!("It then waits for {}.", rest.join(", ")),
    }
}

impl App {
    /// The runs waiting at a point, oldest first, as `GET /runs/waiting` answered for the caller:
    /// those they may see, each point with the steps they may take (`yours`).
    pub(crate) fn runs_waiting(&self) -> Vec<Value> {
        self.client.as_ref().map(|c| c.waiting_runs.clone()).unwrap_or_default()
    }

    /// How many waiting runs the caller may decide a step of now: what waits for them of the runs.
    pub(crate) fn runs_yours(&self) -> usize {
        self.runs_waiting().iter().filter(|r| yours(r).is_some()).count()
    }

    /// Go to the Approvals page's runs, the waiting run `run_id` picked (the first when it isn't
    /// there).
    pub(crate) fn show_waiting_run(&mut self, run_id: Option<&str>) {
        self.go(super::app::Page::Approvals);
        self.approvals_part = Part::Runs;
        self.run_sel = run_id.and_then(|id| self.runs_waiting().iter().position(|r| r["run_id"].as_str() == Some(id))).unwrap_or(0);
    }

    /// A key on the Approvals page's runs: true when taken.
    pub(crate) fn runs_key(&mut self, k: KeyEvent) -> bool {
        let n = self.runs_waiting().len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.run_sel = self.run_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.run_sel = (self.run_sel + 1).min(n - 1),
            KeyCode::Char(']') if n > 0 => self.run_sel = (self.run_sel + 1) % n,
            KeyCode::Char('[') if n > 0 => self.run_sel = (self.run_sel + n - 1) % n,
            KeyCode::Home => self.run_sel = 0,
            KeyCode::End => self.run_sel = n.saturating_sub(1),
            KeyCode::Char('a') => self.ask_decision(true),
            KeyCode::Char('d') => self.ask_decision(false),
            KeyCode::Enter => {
                let slug = self.runs_waiting().get(self.run_sel).and_then(|r| r["slug"].as_str().map(String::from));
                if let Some(slug) = slug {
                    self.open_flow(&slug);
                }
            }
            _ => return false,
        }
        true
    }

    /// Ask the picked run's decision: approving, a reason may come with it; declining, it must.
    /// The dialog says what is decided, what the point shows, and what the run waits for then.
    fn ask_decision(&mut self, approve: bool) {
        let runs = self.runs_waiting();
        let Some(run) = runs.get(self.run_sel.min(runs.len().saturating_sub(1))) else { return self.fail("No run waits for a person.") };
        let Some((w, slot)) = yours(run) else {
            let why = self.client.as_ref().and_then(|c| c.may("run.decide", run["slug"].as_str()).err());
            return self.fail(why.unwrap_or_else(|| "None of its steps is yours to take now: it waits for someone else, or for a step before it.".into()));
        };
        let (slug, run_id, name) = (run["slug"].as_str().unwrap_or("?").to_string(), run["run_id"].as_str().unwrap_or("?").to_string(), w["name"].as_str().unwrap_or("?").to_string());
        let route = run["route"].as_str().or(run["kind"].as_str()).unwrap_or("?");
        let verb = if approve { "Approve" } else { "Decline" };
        let title = format!("{verb} {name:?}{} for run {} of {slug} ({route})?", as_role(w, slot), crate::decidecmd::short(&run_id));
        let mut lines = shown(w);
        if approve {
            lines.push(then(w, slot));
            lines.push("A reason is optional: it is kept with the run.".into());
        } else {
            lines.push("Why: it is kept with the run, and the run goes on as declined.".into());
        }
        let decision = Decision { slug, run_id, name, slot, approve };
        self.modal = Some(Modal::input(title, lines, String::new(), false, Ask::Decide(decision)));
    }

    /// Ask why a proposed version is declined: its author reads it, through holon mcp when an agent.
    pub(crate) fn ask_decline(&mut self, p: Proposed) {
        let reads = if p.agent { ", and the agent reads it through holon mcp" } else { "" };
        let lines = vec![
            format!("{} v{}, proposed by {}. Tell them what to change.", p.slug, p.version, p.by),
            format!("It is kept on the flow's history{reads}. The version stays saved, and what runs keeps running."),
        ];
        self.modal = Some(Modal::input("Why it was declined".into(), lines, String::new(), false, Ask::Decline(p)));
    }

    /// Decline a proposed version, saying why.
    pub(crate) fn decline(&mut self, p: Proposed, reason: String) {
        let Some(c) = self.client.as_ref() else { return };
        match c.post(&format!("/proposals/{}/decline", p.id), json!({ "reason": reason.trim() })) {
            Ok(_) => {
                self.approval = None;
                self.refresh();
                self.say(format!("Declined {} v{}.{}", p.slug, p.version, if p.agent { " The agent gets your note." } else { "" }));
            }
            Err(e) => self.fail(e),
        }
    }

    /// The access changes, newest first, as `GET /access/proposals` answered: those waiting, then
    /// the rest.
    pub(crate) fn access_changes(&self) -> Vec<Value> {
        self.client.as_ref().map(|c| c.access_changes.clone()).unwrap_or_default()
    }

    /// The access changes waiting for people, newest first.
    pub(crate) fn changes_waiting(&self) -> Vec<Value> {
        self.access_changes().into_iter().filter(|p| p["state"] == "waiting").collect()
    }

    /// How many access changes wait for the caller: waiting, theirs to approve, and they may.
    pub(crate) fn changes_yours(&self) -> usize {
        let Some(c) = self.client.as_ref() else { return 0 };
        if c.may("people.assign", None).is_err() {
            return 0;
        }
        let me = c.me.clone().unwrap_or_default();
        self.changes_waiting().iter().filter(|p| not_yours(p, &me).is_none()).count()
    }

    /// Everything waiting for the caller: versions and flows, steps of runs, access changes.
    pub(crate) fn waiting_total(&self) -> usize {
        self.waiting().len() + self.runs_yours() + self.changes_yours()
    }

    /// A key on the Approvals page's access changes: true when taken.
    pub(crate) fn access_key(&mut self, k: KeyEvent) -> bool {
        let n = self.changes_waiting().len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.change_sel = self.change_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.change_sel = (self.change_sel + 1).min(n - 1),
            KeyCode::Char(']') if n > 0 => self.change_sel = (self.change_sel + 1) % n,
            KeyCode::Char('[') if n > 0 => self.change_sel = (self.change_sel + n - 1) % n,
            KeyCode::Home => self.change_sel = 0,
            KeyCode::End => self.change_sel = n.saturating_sub(1),
            KeyCode::Char('a') => self.ask_change(true),
            KeyCode::Char('d') => self.ask_change(false),
            _ => return false,
        }
        true
    }

    /// Ask the picked access change's decision, as the web asks it: approving, what it changes and
    /// gives first, then who proposed it and why; declining, why, which its author reads; its
    /// author withdraws it so.
    fn ask_change(&mut self, approve: bool) {
        let changes = self.changes_waiting();
        let Some(p) = changes.get(self.change_sel.min(changes.len().saturating_sub(1))).cloned() else { return self.fail("No access change waits.") };
        let Some(c) = self.client.as_ref() else { return };
        if let Err(why) = c.may("people.assign", None) {
            return self.fail(format!("Deciding an access change takes people.assign: {why}"));
        }
        let me = c.me.clone().unwrap_or_default();
        let (id, what, author) = (p["id"].as_u64().unwrap_or(0), p["what"].as_str().unwrap_or("?").to_string(), p["author"].as_str().unwrap_or("someone").to_string());
        if approve {
            if let Some(why) = not_yours(&p, &me) {
                return self.fail(format!("Access change {id} isn't yours to approve: {why}."));
            }
            let more = p["needs"].as_u64().unwrap_or(1).saturating_sub(p["approvals"].as_array().map_or(0, |a| a.len() as u64));
            let then = if more > 1 { format!("It then waits for {} more {}.", more - 1, if more == 2 { "person" } else { "people" }) } else { "It is made once you do, checked again then.".into() };
            let why = p["why"].as_str().filter(|w| !w.trim().is_empty()).map(|w| format!(", saying: {}", clipped(w, WHY_CHARS))).unwrap_or_default();
            let lines = vec![format!("{what}. It gives {}.", gives(&p)), format!("Proposed by {author}{why}. {then}")];
            self.modal = Some(Modal::Confirm { title: format!("Approve access change {id}?"), lines, then: super::app::Then::AccessApprove(id) });
            return;
        }
        let withdraw = author == me;
        let (title, lines) = if withdraw {
            (format!("Withdraw access change {id}?"), vec![format!("{what}. Why: it is kept with it.")])
        } else {
            (format!("Decline access change {id}?"), vec![format!("{what}, proposed by {author}. Why: it is kept with it, for them to read.")])
        };
        self.modal = Some(Modal::input(title, lines, String::new(), false, Ask::AccessDecline { id, withdraw }));
    }

    /// Approve an access change, or decline it saying why; then say what that did and read the
    /// list again (a stale one, the server's 409, is said and gone from it).
    pub(crate) fn decide_change(&mut self, id: u64, approve: bool, reason: String) {
        let Some(c) = self.client.as_ref() else { return };
        let (verb, body) = if approve { ("approve", json!({})) } else { ("decline", json!({ "reason": reason.trim() })) };
        match c.post(&format!("/access/proposals/{id}/{verb}"), body) {
            Ok(v) => self.say(crate::accesscmd::decided_line(&v["proposal"])),
            Err(e) => self.fail(e),
        }
        let note = self.note.take();
        self.refresh();
        self.note = note.or(self.note.take());
    }

    /// Send a decision, then say what it did and read the runs again.
    pub(crate) fn decide(&mut self, d: Decision, reason: String) {
        let Some(c) = self.client.as_ref() else { return };
        let mut body = json!({ "name": d.name, "approve": d.approve, "slot": d.slot });
        if !reason.trim().is_empty() {
            body["reason"] = json!(reason.trim());
        }
        match c.post(&format!("/deployments/{}/runs/{}/decide", seg(&d.slug), seg(&d.run_id)), body) {
            Ok(answer) => {
                let said = crate::decidecmd::decided_said(&d.slug, &d.run_id, &answer);
                match answer["run_error"].as_str() {
                    Some(e) => self.fail(format!("{said} The run then failed: {e}")),
                    None => self.say(said),
                }
            }
            Err(e) => self.fail(e),
        }
        self.refresh();
    }
}
