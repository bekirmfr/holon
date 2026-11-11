//! Proposals: versions someone asks a person to deploy (gaps-web.md §7). An agent's deploy that
//! needs a person's approval is kept as one (deploy.rs), with its author's why and note, what it
//! asked to be approved, and what became of it: approved when a person deploys its unit, declined
//! with a reason, or replaced by a newer proposal of the same flow. One waits per flow at most.
//!
//! Kept beside the saved flows (`flows.proposals.json`), written through as they are. A deployed
//! flow's timeline names each too (events.rs, kind `"proposal"`).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::*;
use crate::access::Caller;
use crate::can::{can, Action, Resource};

/// What became of a proposal.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Decision {
    Waiting,
    Approved,
    Declined,
    /// A newer proposal of the same flow came before a person decided.
    Replaced,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) id: u64,
    pub(crate) slug: String,
    pub(crate) unit: String,
    /// One line from its author: what it changes, and why.
    pub(crate) why: String,
    /// More, when its author has more to say (what they tried, who asked for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) by: Option<String>,
    pub(crate) agent: bool,
    pub(crate) at_ms: u64,
    /// What it asked a person to approve when it was made (the deploy's `acknowledge` items).
    pub(crate) needs: Vec<String>,
    /// The stamp of the save it made of the flow (`FlowRecord::saved_at_epoch_seconds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) saved_at: Option<u64>,
    pub(crate) state: Decision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) decided_at_ms: Option<u64>,
    /// Why it was declined, for its author to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    /// Its canvas, as deployed it would be (with its text, `source`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) document: Option<Value>,
}

/// How many decided proposals `GET /proposals` lists, newest first.
const DECIDED: usize = 50;

#[derive(Debug, Default)]
pub(crate) struct Proposals {
    /// Where they are kept (none: memory only).
    path: Option<PathBuf>,
    kept: Mutex<Vec<Proposal>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Proposals {
    /// The proposals kept beside the saved flows at `flows_path`, read back. A file that can't be
    /// read is an error: the next write would lose every proposal in it.
    pub(crate) fn beside(flows_path: &Place) -> Result<Proposals, String> {
        let path = flows_path.beside("proposals.json").map(PathBuf::from);
        let kept = match &path {
            None => Vec::new(),
            Some(p) => match std::fs::read(p) {
                Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{p:?} is not valid proposals JSON: {e}"))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(format!("could not read {p:?}: {e}")),
            },
        };
        Ok(Proposals { path, kept: Mutex::new(kept) })
    }

    fn persist(&self, kept: &[Proposal]) {
        let Some(path) = &self.path else { return };
        match serde_json::to_vec_pretty(kept) {
            Ok(bytes) => {
                if let Err(e) = write_atomically(path, &bytes) {
                    eprintln!("warning: failed to persist proposals to {path:?}: {e}");
                }
            }
            Err(e) => eprintln!("warning: failed to serialize proposals: {e}"),
        }
    }

    /// Keep `proposal` (its id and state are given here); the one waiting for the same flow, if
    /// any, is replaced.
    fn add(&self, mut proposal: Proposal) -> Proposal {
        let mut kept = self.kept.lock().unwrap();
        for p in kept.iter_mut().filter(|p| p.slug == proposal.slug && p.state == Decision::Waiting) {
            p.state = Decision::Replaced;
            p.decided_at_ms = Some(proposal.at_ms);
        }
        proposal.id = kept.iter().map(|p| p.id).max().map_or(1, |n| n + 1);
        proposal.state = Decision::Waiting;
        kept.push(proposal.clone());
        self.persist(&kept);
        proposal
    }

    /// A person deployed `unit` as `slug`: the proposal of it that waits is approved.
    pub(crate) fn approved(&self, slug: &str, unit: &str, by: Option<&str>) {
        let mut kept = self.kept.lock().unwrap();
        let mut changed = false;
        for p in kept.iter_mut().filter(|p| p.slug == slug && p.unit == unit && p.state == Decision::Waiting) {
            p.state = Decision::Approved;
            p.decided_by = by.map(String::from);
            p.decided_at_ms = Some(now_ms());
            changed = true;
        }
        if changed {
            self.persist(&kept);
        }
    }

    /// The waiting proposal `id`, declined by `by` for `reason`.
    fn decline(&self, id: u64, by: Option<&str>, reason: Option<String>) -> Result<Proposal, Response> {
        let mut kept = self.kept.lock().unwrap();
        let Some(p) = kept.iter_mut().find(|p| p.id == id) else { return Err(err(404, format!("no proposal {id}"))) };
        if p.state != Decision::Waiting {
            return Err(err(409, format!("proposal {id} isn't waiting: it was {}", word(p.state))));
        }
        p.state = Decision::Declined;
        p.decided_by = by.map(String::from);
        p.decided_at_ms = Some(now_ms());
        p.reason = reason;
        let declined = p.clone();
        self.persist(&kept);
        Ok(declined)
    }

    /// Every proposal, as kept (oldest first).
    pub(crate) fn all(&self) -> Vec<Proposal> {
        self.kept.lock().unwrap().clone()
    }
}

fn word(d: Decision) -> &'static str {
    match d {
        Decision::Waiting => "waiting",
        Decision::Approved => "approved",
        Decision::Declined => "declined",
        Decision::Replaced => "replaced by a newer one",
    }
}

/// The number `slug`'s next deploy would make.
fn next_version(state: &AppState, slug: &str) -> u32 {
    state.deployments.lock().unwrap().get(slug).and_then(|d| d.versions.last()).map_or(1, |v| v.number + 1)
}

/// A proposal as listed: without its document, with the version it would deploy as while it waits.
fn listed(state: &AppState, p: &Proposal, with_document: bool) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    if !with_document && let Some(o) = v.as_object_mut() {
        o.remove("document");
    }
    if p.state == Decision::Waiting {
        v["version"] = json!(next_version(state, &p.slug));
    }
    v
}

/// Put `what` on `slug`'s timeline, when it is deployed.
fn on_timeline(state: &AppState, slug: &str, what: String, by: Option<&str>) {
    if let Some(dep) = state.deployments.lock().unwrap().get_mut(slug) {
        let seq = dep.log.next_seq();
        dep.events.push("proposal", what, by, seq, crate::events::Detail::default());
    }
}

/// An agent's deploy of `body` that needs a person to approve `needs`: the flow saved under the
/// agent's name (never over someone else's), and kept as a proposal. 202, with the proposal.
pub(crate) fn propose(state: &AppState, body: &DeployRequestBody, needs: Vec<String>, who: &crate::access::Caller) -> Response {
    let by = who.name.as_deref();
    let Some(document) = body.document.clone() else {
        return err(400, "a proposal keeps its canvas for the person who reviews it: send \"document\"");
    };
    let saved_at = match crate::flows::save(state, &body.slug, document.clone(), body.uses.clone(), who) {
        Ok(at) => at,
        Err(refused) => return refused,
    };
    let why = body.why.as_deref().map(str::trim).filter(|w| !w.is_empty()).unwrap_or("").to_string();
    let note = body.note.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(String::from);
    let proposal = state.proposals.add(Proposal {
        id: 0,
        slug: body.slug.clone(),
        unit: body.unit.clone(),
        why,
        note,
        by: by.map(String::from),
        agent: who.kind == Some(crate::accounts::Kind::Agent),
        at_ms: now_ms(),
        needs,
        saved_at: Some(saved_at),
        state: Decision::Waiting,
        decided_by: None,
        decided_at_ms: None,
        reason: None,
        document: Some(document),
    });
    let version = next_version(state, &body.slug);
    let said = if proposal.why.is_empty() { String::new() } else { format!(": {}", proposal.why) };
    on_timeline(state, &body.slug, format!("version {version} proposed{said}"), by);
    state.activity.record("proposed", &body.slug, by);
    Response::json(&json!({ "proposal": listed(state, &proposal, false) })).with_status_code(202)
}

/// Whether `who` may view the flow `p` proposes a version of.
fn views(who: &Caller, p: &Proposal) -> bool {
    can(who, Action::FlowView, &Resource::Flow(&p.slug)).is_ok()
}

/// `GET /proposals`: of the flows `who` may view, the waiting ones, oldest first (the queue a
/// person goes through), then the decided ones, newest first, the last `DECIDED`.
pub(crate) fn handle_get_proposals(state: &AppState, who: &Caller) -> Response {
    let all = state.proposals.all();
    let waiting = all.iter().filter(|p| p.state == Decision::Waiting && views(who, p));
    let decided = all.iter().rev().filter(|p| p.state != Decision::Waiting && views(who, p)).take(DECIDED);
    let listed: Vec<Value> = waiting.chain(decided).map(|p| listed(state, p, false)).collect();
    Response::json(&json!({ "proposals": listed }))
}

/// `GET /proposals/<id>`: one, with its document; none of a flow `who` may not view.
pub(crate) fn handle_get_proposal(state: &AppState, id: &str, who: &Caller) -> Response {
    match id.parse::<u64>().ok().and_then(|id| state.proposals.all().into_iter().find(|p| p.id == id && views(who, p))) {
        Some(p) => Response::json(&listed(state, &p, true)),
        None => err(404, format!("no proposal {id}")),
    }
}

#[derive(Deserialize, Default)]
struct DeclineBody {
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /proposals/<id>/decline`, `{ "reason" }`: a person who approves its flow's deploys
/// declines it (can.rs keeps agents and viewers out). It stays saved, and what runs keeps
/// running; its author reads the reason.
pub(crate) fn handle_post_decline(state: &AppState, id: &str, request: &Request, who: &Caller) -> Response {
    let by = who.name.as_deref();
    let Ok(id) = id.parse::<u64>() else { return err(404, format!("no proposal {id}")) };
    // A proposal of a flow they may not view is none; one they view but don't approve, refused.
    match state.proposals.all().into_iter().find(|p| p.id == id && views(who, p)) {
        None => return err(404, format!("no proposal {id}")),
        Some(p) => {
            if let Err(refusal) = can(who, Action::FlowApproveDeploy, &Resource::Flow(&p.slug)) {
                return refusal.response();
            }
        }
    }
    let body: DeclineBody = match read_body(request) {
        Ok(b) if b.is_empty() => DeclineBody::default(),
        Ok(b) => match serde_json::from_slice(&b) {
            Ok(body) => body,
            Err(e) => return err(400, format!("not a valid decline JSON: {e}")),
        },
        Err(r) => return r,
    };
    let reason = body.reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    let declined = match state.proposals.decline(id, by, reason) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let version = next_version(state, &declined.slug);
    let because = declined.reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default();
    on_timeline(state, &declined.slug, format!("version {version} declined{because}"), by);
    state.activity.record("declined", &declined.slug, by);
    Response::json(&listed(state, &declined, false))
}

#[derive(Deserialize)]
struct SettingsBody {
    agents_propose: bool,
}

/// `POST /deployments/<slug>/settings`, `{ "agents_propose" }`: whether agents may propose its
/// versions. Off, an agent's deploy of it is refused (deploy.rs). Kept as an event.
pub(crate) fn handle_post_settings(state: &AppState, slug: &str, request: &Request, by: Option<&str>) -> Response {
    let body: SettingsBody = match read_body(request).map(|b| serde_json::from_slice(&b)) {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => return err(400, format!("not a valid settings JSON: {e}")),
        Err(r) => return r,
    };
    let entry = {
        let mut deployments = state.deployments.lock().unwrap();
        let Some(dep) = deployments.get_mut(slug) else { return err(404, format!("no deployment {slug:?}")) };
        if dep.agents_propose_off != body.agents_propose {
            return Response::json(&crate::deploy::deployment_entry(slug, dep));
        }
        dep.agents_propose_off = !body.agents_propose;
        let what = if body.agents_propose { "agents may propose versions" } else { "agents may no longer propose versions" };
        let seq = dep.log.next_seq();
        dep.events.push("settings", what.into(), by, seq, crate::events::Detail::default());
        crate::deploy::deployment_entry(slug, dep)
    };
    persist_deployments(&state.deployments_path, &state.deployments);
    Response::json(&entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(slug: &str, unit: &str) -> Proposal {
        Proposal {
            id: 0,
            slug: slug.into(),
            unit: unit.into(),
            why: "calls httpbin".into(),
            note: None,
            by: Some("claude".into()),
            agent: true,
            at_ms: 1,
            needs: vec!["NetworkEgress(\"httpbin.org\")".into()],
            saved_at: Some(1),
            state: Decision::Waiting,
            decided_by: None,
            decided_at_ms: None,
            reason: None,
            document: None,
        }
    }

    #[test]
    fn a_newer_proposal_replaces_the_one_waiting_and_a_deploy_of_its_unit_approves_it() {
        let kept = Proposals::default();
        let first = kept.add(proposal("a", "u1"));
        let other = kept.add(proposal("b", "u9"));
        let second = kept.add(proposal("a", "u2"));
        assert_eq!((first.id, other.id, second.id), (1, 2, 3));
        let state = |id: u64| kept.all().into_iter().find(|p| p.id == id).unwrap().state;
        assert_eq!((state(1), state(2), state(3)), (Decision::Replaced, Decision::Waiting, Decision::Waiting));
        // A deploy of another unit approves nothing; of its unit, it.
        kept.approved("a", "u1", Some("mira"));
        assert_eq!(state(3), Decision::Waiting);
        kept.approved("a", "u2", Some("mira"));
        assert_eq!(state(3), Decision::Approved);
        assert_eq!(kept.all()[2].decided_by.as_deref(), Some("mira"));
    }

    #[test]
    fn a_proposal_declined_keeps_its_reason_and_is_declined_once() {
        let kept = Proposals::default();
        let p = kept.add(proposal("a", "u1"));
        let declined = kept.decline(p.id, Some("mira"), Some("use the sandbox".into())).unwrap();
        assert_eq!((declined.state, declined.reason.as_deref()), (Decision::Declined, Some("use the sandbox")));
        assert_eq!(kept.decline(p.id, Some("mira"), None).unwrap_err().status_code, 409);
        assert_eq!(kept.decline(99, Some("mira"), None).unwrap_err().status_code, 404);
    }

    #[test]
    fn proposals_are_read_back_from_beside_the_flows() {
        let dir = std::env::temp_dir().join(format!("holon-proposals-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let flows = Place::parse(dir.join("flows.json").to_str().unwrap());
        Proposals::beside(&flows).unwrap().add(proposal("a", "u1"));
        let again = Proposals::beside(&flows).unwrap();
        assert_eq!(again.all().len(), 1);
        assert_eq!(again.all()[0].why, "calls httpbin");
        std::fs::write(dir.join("flows.proposals.json"), b"not json").unwrap();
        assert!(Proposals::beside(&flows).is_err(), "an unreadable file refuses rather than losing them");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
