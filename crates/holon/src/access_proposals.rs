//! Access changes that wait for people (docs/L4s/access-and-approvals-L4.md §6; the access plan's
//! 8c, D4 to D7). While the host's `approvals.changes` is N (bindings.rs `ApprovalSettings`), a
//! change to what is bound that gives someone what they didn't hold of `run.decide` or of an
//! admin's (people, roles, secrets, the host's resources and operations), whether by a binding, a
//! group they join, a role changed to hold it or a role set on the host, or that loosens how
//! approvals are decided, isn't made: it is kept as a proposal until N people other than its
//! author approve it. Each is a person holding people.assign on the host and what the change
//! gives (D6: two who hold less don't give more together). The N-th approval makes it as its
//! author asked it, checked again against the document as it is then: one that no longer applies
//! (its role gone, the last admin) is stale, saying why, and its author proposes it again.
//!
//! Taking away applies at once (D4), as does a change that gives no one more, and a change to the
//! approval settings made through this machine's run file (§6, no one locks the host out: the OS
//! user holds the data folder anyway, and it is how a host whose approvers are gone gets its
//! access back). A new principal is never made waiting: an invitation's link is shown once, to
//! whoever makes it, so one with a role that would wait is refused, saying to give the role after.
//!
//! Proposals are a file of their own beside the access document, `access.proposals.json` (D7): a
//! proposal or an approval changes no one's access, and the document's history keeps what did.
//! Made, a proposal's change is a version like any other, by its author, its why saying who
//! approved it.
//!
//! The routes (bindings.rs's `route` hands them here; can.rs's table asks who may):
//! - `GET /access/proposals`: every proposal, newest first (people.view);
//! - `POST /access/proposals/<id>/approve`: approved, and made if it is the last approval it
//!   waits for (people.assign);
//! - `POST /access/proposals/<id>/decline` (`{reason}`): declined; its author may withdraw it
//!   (people.assign).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rouille::{Request, Response};
use serde::{Deserialize, Serialize};

use crate::access::{Caller, Role};
use crate::accounts::{Accounts, Kind};
use crate::bindings::{self, ApprovalSettings, Document, Group, Held, RoleText, Scope, Subject};
use crate::can::{can, Action, Resource};
use crate::deploy::epoch_now;
use crate::http::{err, read_body};
use crate::sync::Mutex;

/// An access change as its route got it, kept so that it is shown and made as asked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Change {
    /// `POST /bindings`.
    Bind {
        who: Subject,
        role: String,
        scope: Scope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
    /// `PUT /roles/<name>`: the role whole, as it would be.
    Role { role: RoleText },
    /// `PUT /groups/<name>`: the group whole, its members all of them.
    Group { group: Group },
    /// `PATCH /people/<name>` with a role: bound on the host in place of what is bound there.
    HostRole { name: String, role: String },
    /// `PUT /access/approvals`: the settings whole, as they would be.
    Approvals { settings: ApprovalSettings },
}

impl Change {
    /// Make it, by `by`, saying `why`, as its route would.
    pub(crate) fn make(&self, accounts: &Accounts, by: &str, why: Option<&str>) -> Result<(), String> {
        match self.clone() {
            Change::Bind { who, role, scope, expires_at } => accounts.bind(who, &role, scope, expires_at, by, why).map(drop),
            Change::Role { role } => accounts.define_role(role, by, why).map(drop),
            Change::Group { group } => accounts.set_group(group, by, why).map(drop),
            Change::HostRole { name, role } => accounts.set_role(&name, &role, by, why),
            Change::Approvals { settings } => accounts.set_approvals(settings, by, why).map(drop),
        }
    }
}

/// Whether giving `action` waits for people (D4): `run.decide`, and what is an admin's (every
/// action the built-in editor doesn't hold).
pub(crate) fn governed(action: Action) -> bool {
    action == Action::RunDecide || !crate::can::role(Role::Editor).allows().contains(&action)
}

/// An access change waiting for people, or what became of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub(crate) id: u64,
    pub(crate) author: String,
    /// When it was proposed (seconds).
    pub(crate) at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
    pub(crate) change: Change,
    /// What it changes, as its version will say it: "bound approver to eve on this host".
    pub(crate) what: String,
    /// What it gives, in words: whoever approves it holds all of it (D6).
    pub(crate) gives: Vec<String>,
    /// The access version it was proposed on.
    pub(crate) base: u64,
    /// How many approve it: the host's setting when it was proposed, or since, if that is more.
    pub(crate) needs: u8,
    pub(crate) approvals: Vec<Approval>,
    #[serde(flatten)]
    pub(crate) state: State,
    /// When it was made, declined or found stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ended_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Approval {
    pub(crate) who: String,
    pub(crate) at: u64,
}

/// On disk and in an answer, beside the proposal's fields: `"state": "waiting"`;
/// `"state": "applied", "version": 7`; `"state": "declined", "declined_by": "bob", "reason": …`;
/// `"state": "stale", "stale": "no role \"approver\""`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub(crate) enum State {
    Waiting,
    /// Made, as this access version.
    Applied { version: u64 },
    Declined { declined_by: String, reason: String },
    /// It no longer applies, and why.
    Stale { stale: String },
}

impl State {
    /// As a refusal says it: "was declined by bob: not this week".
    fn said(&self) -> String {
        match self {
            State::Waiting => "waits".into(),
            State::Applied { version } => format!("was made, as access version {version}"),
            State::Declined { declined_by, reason } => format!("was declined by {declined_by}: {reason}"),
            State::Stale { stale } => format!("is stale: {stale}"),
        }
    }
}

/// Where the proposals for the access document at `access` are kept: `access.proposals.json`.
pub(crate) fn beside(access: &Path) -> PathBuf {
    access.with_extension("proposals.json")
}

/// The proposals, every one, as the server holds them. Its lock is taken before the accounts
/// book's, by `govern`, `approve` and `decline`: one at a time, each reads what a change gives
/// against the document it is then made to.
pub(crate) struct Proposals {
    /// None in memory (the tests, and a change tried before it is made: accounts.rs `trial`).
    path: Option<PathBuf>,
    list: Mutex<Vec<Proposal>>,
}

impl Proposals {
    /// The proposals at `path`: none yet if it isn't there.
    pub(crate) fn open(path: PathBuf) -> Result<Proposals, String> {
        let list = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("the access proposals {path:?}: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("the access proposals {path:?}: {e}")),
        };
        Ok(Proposals { path: Some(path), list: Mutex::new(list) })
    }

    pub(crate) fn memory() -> Proposals {
        Proposals { path: None, list: Mutex::new(Vec::new()) }
    }

    fn save(&self, list: &[Proposal]) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
        crate::sealing::write_private(path, &text).map_err(|e| format!("the access proposals {path:?}: {e}"))
    }

    /// Every proposal, oldest first.
    pub(crate) fn all(&self) -> Vec<Proposal> {
        self.list.lock().unwrap().clone()
    }
}

/// What a change gives someone they didn't hold: `action` where `role` is bound at `scope`.
#[derive(Clone, Debug, PartialEq)]
struct Gain {
    who: String,
    role: String,
    action: Action,
    scope: Scope,
}

/// What `after` gives that `before` didn't, of what is governed: for each principal here
/// (`kind_of`), each binding reaching it in `after` that holds now, of a role its kind may hold,
/// each governed action it gives where it is bound that the principal may do there by `after` and
/// couldn't by `before` for as long (a binding lasting longer than the one before gives more).
fn gains(before: &Document, after: &Document, kind_of: &dyn Fn(&str) -> Option<Kind>, now: u64) -> Vec<Gain> {
    let (was, is) = (before.index(), after.index());
    let mut names: Vec<&String> = is.keys().collect();
    names.sort();
    let mut gains: Vec<Gain> = Vec::new();
    for name in names {
        let Some(kind) = kind_of(name) else { continue };
        let held = &is[name];
        for (h, end) in held {
            if end.is_some_and(|end| end <= now) || !h.role.kinds.contains(&kind) {
                continue;
            }
            let then = bindings::as_caller(was.get(name).map(Vec::as_slice).unwrap_or_default(), name, kind, now, *end);
            let next = bindings::as_caller(held, name, kind, now, *end);
            for (action, on) in bindings::gives(&h.role, &h.scope) {
                let new = governed(action) && can(&next, action, &on).is_ok() && can(&then, action, &on).is_err();
                if new && !gains.iter().any(|g| g.who == *name && g.action == action && g.scope == h.scope) {
                    gains.push(Gain { who: name.clone(), role: h.role.name.clone(), action, scope: h.scope.clone() });
                }
            }
        }
    }
    gains
}

/// How `after` loosens `before` (D5), each in `after`'s words: whoever deployed a version deciding
/// its approvals again, agents taking their steps again, the starter taking a step that says so
/// again, fewer people approving an access change, or none.
fn loosened(before: &ApprovalSettings, after: &ApprovalSettings) -> Vec<String> {
    let fewer = match (before.changes, after.changes) {
        (Some(was), Some(is)) => is < was,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let looser = [before.exclude_deployer && !after.exclude_deployer, !before.agents && after.agents, !before.starter && after.starter, fewer];
    after.each().into_iter().zip(looser).filter(|(_, looser)| *looser).map(|((_, _, said), _)| said).collect()
}

/// `gains` in words, one for each principal, role and scope: "run.decide to eve on this host, as
/// approver".
fn said(gains: &[Gain]) -> Vec<String> {
    let mut each: Vec<(&Gain, Vec<&str>)> = Vec::new();
    for g in gains {
        match each.iter_mut().find(|(first, _)| first.who == g.who && first.role == g.role && first.scope == g.scope) {
            Some((_, actions)) => actions.push(g.action.name()),
            None => each.push((g, vec![g.action.name()])),
        }
    }
    each.into_iter().map(|(g, actions)| format!("{} to {} {}, as {}", actions.join(", "), g.who, g.scope.said(), g.role)).collect()
}

/// "1 person", "2 people".
fn people(n: u8) -> String {
    if n == 1 { "1 person".into() } else { format!("{n} people") }
}

/// "bob", "bob and cy", "bob, cy and dee".
fn and(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A change made, or kept as a proposal.
pub(crate) enum Governed<T> {
    Made(T),
    Waits(Box<Proposal>),
}

/// Make `change` by `caller` with `make` (its route's own call, answered as it answers), or keep
/// it as a proposal when it gives more and this host has people approve such a change (D4, D5).
/// What it gives is read by making it on a copy first (accounts.rs `trial`), so a change that
/// can't be made is refused as `make` would refuse it, and never proposed. One at a time with
/// every other governed change and approval.
pub(crate) fn govern<T>(accounts: &Accounts, caller: &Caller, change: Change, why: Option<&str>, make: impl FnOnce() -> Result<T, String>) -> Result<Governed<T>, String> {
    let proposals = accounts.proposals();
    let mut list = proposals.list.lock().unwrap();
    let run_file = caller.local.is_some() && matches!(change, Change::Approvals { .. });
    if run_file || accounts.access_document().approvals.changes.is_none() {
        return make().map(Governed::Made);
    }
    let by = caller.name.as_deref().unwrap_or("");
    let (before, after) = accounts.trial(|scratch| change.make(scratch, by, why))?;
    let now = epoch_now();
    let kind_of = |name: &str| accounts.principal(name).map(|p| p.kind);
    let mut gives = said(&gains(&before, &after, &kind_of, now));
    gives.extend(loosened(&before.approvals, &after.approvals));
    let Some(needs) = before.approvals.changes.filter(|_| !gives.is_empty()) else { return make().map(Governed::Made) };
    let id = list.iter().map(|p| p.id).max().unwrap_or(0) + 1;
    let p = Proposal { id, author: by.into(), at: now, why: why.map(String::from), change, what: after.what.clone(), gives, base: before.version, needs, approvals: Vec::new(), state: State::Waiting, ended_at: None };
    list.push(p.clone());
    if let Err(e) = proposals.save(&list) {
        list.pop();
        return Err(e);
    }
    Ok(Governed::Waits(Box::new(p)))
}

/// Why `role` isn't given to `name`, a new principal of `kind`, at once, if it isn't: it gives
/// what waits for people here, and a new principal can't wait (an invitation's link is shown once,
/// to whoever makes it). Made with a role that gives none of it, it is then given the role as
/// anyone is, and that waits.
pub(crate) fn adding(accounts: &Accounts, name: &str, kind: Kind, role: &str) -> Result<(), String> {
    let document = accounts.access_document();
    let (Some(n), Some(def)) = (document.approvals.changes, document.role(role)) else { return Ok(()) };
    let new = bindings::as_caller(&[(Held::new(Arc::clone(&def), Scope::Host), None)], name, kind, epoch_now(), None);
    let given: Vec<&str> = bindings::gives(&def, &Scope::Host).into_iter().filter(|(a, on)| governed(*a) && can(&new, *a, on).is_ok()).map(|(a, _)| a.name()).collect();
    if given.is_empty() {
        return Ok(());
    }
    let make = if kind == Kind::Person { "invite" } else { "add" };
    let approve = if n == 1 { "approves" } else { "approve" };
    Err(format!(
        "{role} gives {} on this host, which waits here until {} other than its giver {approve} it, and a new principal can't wait: {make} {name} as viewer, then give them {role}, which waits",
        given.join(", "),
        people(n)
    ))
}

/// A refusal: its status and words.
type Refused = (u16, String);

/// Who `caller` is, deciding `p` (`verb`: "approved", "declined"), or why they may not: `p` waits,
/// and they are a person signed in as themself holding people.assign on the host (can.rs's table
/// asks that last of the routes; it is asked here too, of whoever calls this).
fn decider(caller: &Caller, p: &Proposal, verb: &str) -> Result<String, Refused> {
    if p.state != State::Waiting {
        return Err((409, format!("access change {} {}: only one that waits is {verb}", p.id, p.state.said())));
    }
    let name = caller.name.clone().unwrap_or_default();
    if caller.untied() {
        return Err((403, format!("{name} is this machine's run file, tied to no account: an access change is {verb} by a person signed in as themself")));
    }
    if caller.kind != Some(Kind::Person) {
        return Err((403, format!("{name} is {}: an access change is {verb} by a person", caller.kind.map_or("no one", Kind::said))));
    }
    can(caller, Action::PeopleAssign, &Resource::Host).map_err(|r| (403, r.0))?;
    Ok(name)
}

/// `caller` approves the proposal `id`: it, made if theirs is the last approval it waits for, or
/// stale if it no longer applies. Refused (403) its author, and whoever doesn't hold what it gives
/// where it gives it (D6), read against the document as it is now; 409 for a second approval by
/// the same person.
pub(crate) fn approve(accounts: &Accounts, caller: &Caller, id: u64) -> Result<Proposal, Refused> {
    let proposals = accounts.proposals();
    let mut list = proposals.list.lock().unwrap();
    let p = list.iter_mut().find(|p| p.id == id).ok_or_else(|| (404, format!("no access change {id}")))?;
    let name = decider(caller, p, "approved")?;
    if p.author == name {
        return Err((403, format!("{name} proposed access change {id}: it is approved by people other than its author")));
    }
    if p.approvals.iter().any(|a| a.who == name) {
        return Err((409, format!("{name} approved access change {id} already: it waits for {} more", usize::from(p.needs).saturating_sub(p.approvals.len()))));
    }
    let now = epoch_now();
    // Read again against the document as it is now: its author still here, and the change one
    // that can still be made.
    let tried = match accounts.principal(&p.author) {
        Some(author) if !author.disabled => accounts.trial(|scratch| p.change.make(scratch, &p.author, None)),
        _ => Err(format!("its author, {}, is no longer here, or is disabled", p.author)),
    };
    let (before, after) = match tried {
        Ok(tried) => tried,
        Err(e) => {
            p.state = State::Stale { stale: e };
            p.ended_at = Some(now);
            let p = p.clone();
            proposals.save(&list).map_err(|e| (500, e))?;
            return Ok(p);
        }
    };
    let kind_of = |name: &str| accounts.principal(name).map(|p| p.kind);
    if let Some(g) = gains(&before, &after, &kind_of, now).into_iter().find(|g| can(caller, g.action, &bindings::given_on(g.action, &g.scope)).is_err()) {
        return Err((403, format!("access change {id} gives {} to {} {}, which you don't hold: an access change is approved only by someone who holds what it gives", g.action.name(), g.who, g.scope.said())));
    }
    p.approvals.push(Approval { who: name, at: now });
    p.needs = p.needs.max(before.approvals.changes.unwrap_or(0));
    if p.approvals.len() >= usize::from(p.needs) {
        let who: Vec<&str> = p.approvals.iter().map(|a| a.who.as_str()).collect();
        let why = format!("{}access change {id}, approved by {}", p.why.as_ref().map(|w| format!("{w}; ")).unwrap_or_default(), and(&who));
        p.state = match p.change.make(accounts, &p.author, Some(&why)) {
            Ok(()) => State::Applied { version: accounts.access_document().version },
            Err(e) => State::Stale { stale: e },
        };
        p.ended_at = Some(now);
    }
    let p = p.clone();
    proposals.save(&list).map_err(|e| (500, e))?;
    Ok(p)
}

/// `caller` declines the proposal `id`, saying why: it. Its author may withdraw it so; anyone
/// else who may approve it may decline it.
pub(crate) fn decline(accounts: &Accounts, caller: &Caller, id: u64, reason: &str) -> Result<Proposal, Refused> {
    let proposals = accounts.proposals();
    let mut list = proposals.list.lock().unwrap();
    let p = list.iter_mut().find(|p| p.id == id).ok_or_else(|| (404, format!("no access change {id}")))?;
    let name = decider(caller, p, "declined")?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err((400, "say why it is declined: {\"reason\": \"…\"}".into()));
    }
    p.state = State::Declined { declined_by: name, reason: reason.into() };
    p.ended_at = Some(epoch_now());
    let p = p.clone();
    proposals.save(&list).map_err(|e| (500, e))?;
    Ok(p)
}

/// A change kept as the proposal `p`, as its route answers it: 202, with the proposal and the
/// access version it waits on, and in the activity log.
pub(crate) fn waits(state: &crate::AppState, accounts: &Accounts, p: &Proposal) -> Response {
    state.activity.record("proposed an access change", &format!("{}: {}", p.id, p.what), Some(&p.author));
    Response::json(&serde_json::json!({ "proposal": p, "version": accounts.access_document().version })).with_status_code(202)
}

/// `/access/proposals…`.
pub(crate) fn route(state: &crate::AppState, request: &Request, caller: &Caller, accounts: &Accounts) -> Response {
    let url = request.url();
    if url == "/access/proposals" {
        if request.method() != "GET" {
            return Response::empty_404();
        }
        let mut all = accounts.proposals().all();
        all.reverse();
        return Response::json(&serde_json::json!({ "proposals": all, "version": accounts.access_document().version }));
    }
    let rest = url.strip_prefix("/access/proposals/").unwrap_or_default();
    let (id, verb) = rest.split_once('/').unwrap_or((rest, ""));
    let Ok(id) = id.parse::<u64>() else { return err(404, format!("no access change {id:?}")) };
    let decided = match (request.method(), verb) {
        ("POST", "approve") => approve(accounts, caller, id),
        ("POST", "decline") => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Decline {
                #[serde(default)]
                reason: String,
            }
            let shape = |e: serde_json::Error| err(400, format!("a decline is {{\"reason\": \"…\"}}: {e}"));
            match read_body(request).and_then(|b| serde_json::from_slice::<Decline>(&b).map_err(shape)) {
                Ok(d) => decline(accounts, caller, id, &d.reason),
                Err(r) => return r,
            }
        }
        _ => return Response::empty_404(),
    };
    let p = match decided {
        Ok(p) => p,
        Err((status, e)) => return err(status, e),
    };
    let by = caller.name.as_deref();
    let target = format!("{}: {}", p.id, p.what);
    let approved = || state.activity.record(&format!("approved an access change ({} of {})", p.approvals.len(), p.needs), &target, by);
    match &p.state {
        State::Waiting => approved(),
        State::Applied { .. } => {
            approved();
            state.activity.record("made an access change", &target, by);
        }
        State::Declined { declined_by, reason } => {
            let what = if *declined_by == p.author { "withdrew an access change" } else { "declined an access change" };
            state.activity.record(what, &format!("{target} ({reason})"), by);
        }
        State::Stale { stale } => {
            state.activity.record("found an access change stale", &format!("{target} ({stale})"), by);
            return err(409, format!("access change {id} no longer applies, so it is stale: {stale}; its author proposes it again"));
        }
    }
    Response::json(&serde_json::json!({ "proposal": p, "version": accounts.access_document().version }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::Selector;

    /// `name`, a person, holding what is bound to them now.
    fn person(a: &Accounts, name: &str) -> Caller {
        Caller { name: Some(name.into()), kind: Some(Kind::Person), holds: Some(a.holding(name)), local: None }
    }

    fn role(name: &str, allows: &[&str]) -> RoleText {
        RoleText { name: name.into(), about: None, allows: allows.iter().map(|a| a.to_string()).collect(), kinds: vec![Kind::Person] }
    }

    fn pay() -> Scope {
        Scope::Flows(Selector::Slug("pay".into()))
    }

    /// ada and bob, admins; cy holding `assigner` (people.view, people.assign, role.view and
    /// role.define) on the host; dee holding `approver` (run.decide, run.view) on the host, and in
    /// the group `team`, bound approver on pay; eve a viewer holding `watch` (run.view) on pay.
    /// Whoever deployed a version decides none of its approvals, and `changes` people approve an
    /// access change.
    fn universe(changes: Option<u8>) -> Accounts {
        let a = Accounts::memory();
        for (name, role) in [("ada", "admin"), ("bob", "admin"), ("cy", "viewer"), ("dee", "viewer"), ("eve", "viewer")] {
            a.invite(name, role, None, "ada").unwrap();
        }
        for r in [role("assigner", &["people.view", "people.assign", "role.view", "role.define"]), role("approver", &["run.decide", "run.view"]), role("watch", &["run.view"])] {
            a.define_role(r, "ada", None).unwrap();
        }
        let to = |name: &str| Subject::Principal(name.into());
        a.bind(to("cy"), "assigner", Scope::Host, None, "ada", None).unwrap();
        a.bind(to("dee"), "approver", Scope::Host, None, "ada", None).unwrap();
        a.bind(to("eve"), "watch", pay(), None, "ada", None).unwrap();
        a.set_group(Group { name: "team".into(), about: None, members: vec!["dee".into()] }, "ada", None).unwrap();
        a.bind(Subject::Group("team".into()), "approver", pay(), None, "ada", None).unwrap();
        a.set_approvals(ApprovalSettings { exclude_deployer: true, changes, ..ApprovalSettings::default() }, "ada", None).unwrap();
        a
    }

    /// What a change makes of a document: its roles, groups, bindings (but when each was made) and
    /// settings.
    fn kept(d: &Document) -> serde_json::Value {
        let mut bindings = serde_json::to_value(&d.bindings).unwrap();
        for b in bindings.as_array_mut().unwrap() {
            b["at"] = 0.into();
        }
        serde_json::json!({ "roles": d.roles, "groups": d.groups, "bindings": bindings, "approvals": d.approvals })
    }

    /// Each kind of change ada makes to `universe`, with the settings `base`: what it is, whether it
    /// gives more (while the setting is on), and who besides ada holds what it gives, of those who
    /// may approve it (bob, cy; dee holds no people.assign).
    fn changes(base: &ApprovalSettings) -> Vec<(&'static str, Change, bool, &'static [&'static str])> {
        let bind = |role: &str, scope: Scope| Change::Bind { who: Subject::Principal("eve".into()), role: role.into(), scope, expires_at: None };
        let team = |members: &[&str]| Change::Group { group: Group { name: "team".into(), about: None, members: members.iter().map(|m| m.to_string()).collect() } };
        let settings = |f: fn(&mut ApprovalSettings)| {
            let mut s = base.clone();
            f(&mut s);
            Change::Approvals { settings: s }
        };
        vec![
            ("approver bound to eve on the host", bind("approver", Scope::Host), true, &["bob"]),
            ("assigner bound to eve on the host", bind("assigner", Scope::Host), true, &["bob", "cy"]),
            ("approver bound to eve until later", Change::Bind { who: Subject::Principal("dee".into()), role: "approver".into(), scope: pay(), expires_at: Some(epoch_now() + 3600) }, false, &[]),
            ("watch, which eve holds on pay, made to decide", Change::Role { role: role("watch", &["run.view", "run.decide"]) }, true, &["bob"]),
            ("eve into team, bound approver on pay", team(&["dee", "eve"]), true, &["bob"]),
            ("eve made an editor", Change::HostRole { name: "eve".into(), role: "editor".into() }, true, &["bob"]),
            ("whoever deployed a version let decide it", settings(|s| s.exclude_deployer = false), true, &["bob", "cy"]),
            ("one fewer approving an access change", settings(|s| s.changes = s.changes.and_then(|n| n.checked_sub(1)).filter(|n| *n > 0)), true, &["bob", "cy"]),
            ("access changes applying at once", settings(|s| s.changes = None), true, &["bob", "cy"]),
            ("one more approving an access change", settings(|s| s.changes = s.changes.map(|n| n + 1)), false, &[]),
            ("agents kept from their steps", settings(|s| s.agents = false), false, &[]),
            ("watch bound to eve on the host", bind("watch", Scope::Host), false, &[]),
            ("dee out of team", team(&[]), false, &[]),
            ("approver narrowed to run.view", Change::Role { role: role("approver", &["run.view"]) }, false, &[]),
        ]
    }

    /// The model (the access plan's 8c): every kind of change × the setting off, one and two ×
    /// every set of the four who might approve it, each in turn. A change that gives no one more is
    /// made at once, as is any while the setting is off. One that does waits, and is made exactly
    /// when N different people other than its author, each a person holding people.assign on the
    /// host and what it gives, approved it: the document is then what the change makes of the
    /// base, and until then the base. Each refusal is its rule's: its author, and whoever doesn't
    /// hold people.assign or what it gives, 403; an approval once it is made, 409.
    #[test]
    fn an_access_change_is_made_exactly_when_enough_people_holding_what_it_gives_approve_it() {
        let deciders = ["ada", "bob", "cy", "dee"];
        for setting in [None, Some(1), Some(2)] {
            let base = ApprovalSettings { exclude_deployer: true, changes: setting, ..ApprovalSettings::default() };
            for (what, change, gives, holders) in changes(&base) {
                let made = {
                    let m = universe(setting);
                    change.make(&m, "ada", None).unwrap();
                    kept(&m.access_document())
                };
                for subset in 0..1u32 << deciders.len() {
                    let a = universe(setting);
                    let unchanged = kept(&a.access_document());
                    let approvers: Vec<&str> = deciders.iter().enumerate().filter(|(i, _)| subset & (1 << i) != 0).map(|(_, n)| *n).collect();
                    let case = format!("{what}, changes {setting:?}, approved by {approvers:?}");
                    let governed = govern(&a, &person(&a, "ada"), change.clone(), Some("the model"), || change.make(&a, "ada", Some("the model"))).unwrap();
                    let Some(n) = setting.filter(|_| gives) else {
                        assert!(matches!(governed, Governed::Made(())), "{case}: made at once");
                        assert_eq!(kept(&a.access_document()), made, "{case}");
                        continue;
                    };
                    let Governed::Waits(p) = governed else { panic!("{case}: it waits") };
                    assert_eq!((p.needs, &p.state, &p.author), (n, &State::Waiting, &"ada".to_string()), "{case}");
                    assert_eq!(kept(&a.access_document()), unchanged, "{case}: nothing changes while it waits");
                    let mut approved = 0;
                    for who in &approvers {
                        let answer = approve(&a, &person(&a, who), p.id);
                        let want = if approved == n {
                            409
                        } else if *who == "ada" || !holders.contains(who) {
                            403
                        } else {
                            200
                        };
                        assert_eq!(answer.as_ref().map_or_else(|(s, _)| *s, |_| 200), want, "{case}: {who}: {answer:?}");
                        approved += u8::from(want == 200);
                    }
                    let applied = approvers.iter().filter(|w| holders.contains(w)).count() >= usize::from(n);
                    let now = a.proposals().all().into_iter().find(|q| q.id == p.id).unwrap();
                    assert_eq!(matches!(now.state, State::Applied { .. }), applied, "{case}: {now:?}");
                    assert_eq!(kept(&a.access_document()), if applied { made.clone() } else { unchanged }, "{case}");
                    if applied {
                        let why = a.access_document().why.clone().unwrap_or_default();
                        assert!(why.starts_with("the model; access change 1, approved by "), "{case}: {why}");
                    }
                }
            }
        }
    }

    /// What a proposal says it gives, and the settings it loosens, in words; a binding that lasts
    /// longer than one already held gives more, and one that holds no longer gives nothing.
    #[test]
    fn a_proposal_says_what_it_gives() {
        let a = universe(Some(1));
        let ada = person(&a, "ada");
        let soon = epoch_now() + 60;
        a.bind(Subject::Principal("eve".into()), "approver", Scope::Host, Some(soon), "ada", None).unwrap();
        let bind = |role: &str, expires_at: Option<u64>| Change::Bind { who: Subject::Principal("eve".into()), role: role.into(), scope: Scope::Host, expires_at };
        let base = a.access_document().approvals.clone();
        let cases: [(Change, &[&str]); 5] = [
            (bind("approver", Some(soon - 30)), &[]),
            (bind("approver", Some(soon + 30)), &["run.decide to eve on this host, as approver"]),
            (bind("assigner", None), &["people.view, people.assign, role.view, role.define to eve on this host, as assigner"]),
            (Change::HostRole { name: "eve".into(), role: "editor".into() }, &["run.decide to eve on this host, as editor"]),
            (
                Change::Approvals { settings: ApprovalSettings { exclude_deployer: false, changes: None, ..base } },
                &["whoever deployed the version a run is on may decide its approvals", "an access change applies at once"],
            ),
        ];
        for (change, gives) in cases {
            let said = match govern(&a, &ada, change.clone(), None, || change.make(&a, "ada", None)).unwrap() {
                Governed::Made(()) => vec![],
                Governed::Waits(p) => p.gives,
            };
            assert_eq!(said, gives, "{change:?}");
        }
    }

    /// Who may approve or decline, and what is refused, beside the model's: an agent and a service
    /// (403), the run file tied to no account (403), a second approval (409), a proposal not there
    /// (404); a decline needs a reason, its author may withdraw it, and one declined is decided.
    /// A change to the settings through the run file is made at once; one that no longer applies
    /// is stale when approved, the document unchanged.
    #[test]
    fn who_decides_an_access_change_and_what_becomes_of_it() {
        let a = universe(Some(2));
        a.add("bot", Kind::Agent, "agent", "ada", "ada").unwrap();
        a.add("ci", Kind::Service, "viewer", "ada", "ada").unwrap();
        let ada = person(&a, "ada");
        let propose = |change: Change| match govern(&a, &ada, change.clone(), None, || change.make(&a, "ada", None)).unwrap() {
            Governed::Waits(p) => p.id,
            Governed::Made(()) => panic!("{change:?} waits"),
        };
        let eve = |role: &str| Change::Bind { who: Subject::Principal("eve".into()), role: role.into(), scope: Scope::Host, expires_at: None };
        let id = propose(eve("assigner"));
        let status = |r: Result<Proposal, Refused>| r.map_or_else(|(s, e)| (s, e), |p| (200, p.state.said()));
        let as_kind = |name: &str, kind: Kind| Caller { name: Some(name.into()), kind: Some(kind), holds: Some(crate::bindings::Held::admin()), local: None };
        let untied = Caller { local: Some("local:bma".into()), ..as_kind("local:bma", Kind::Person) };
        assert_eq!(status(approve(&a, &as_kind("bot", Kind::Agent), id)).0, 403);
        assert_eq!(status(approve(&a, &as_kind("ci", Kind::Service), id)), (403, "ci is a service: an access change is approved by a person".into()));
        assert_eq!(status(approve(&a, &untied, id)).0, 403);
        assert_eq!(status(approve(&a, &person(&a, "bob"), id)), (200, "waits".into()));
        assert_eq!(status(approve(&a, &person(&a, "bob"), id)), (409, format!("bob approved access change {id} already: it waits for 1 more")));
        assert_eq!(status(approve(&a, &person(&a, "bob"), 99)), (404, "no access change 99".into()));
        assert_eq!(status(decline(&a, &person(&a, "cy"), id, "  ")).0, 400);
        assert_eq!(status(decline(&a, &person(&a, "cy"), id, "not eve")), (200, "was declined by cy: not eve".into()));
        assert_eq!(status(approve(&a, &person(&a, "cy"), id)), (409, format!("access change {id} was declined by cy: not eve: only one that waits is approved")));
        let withdrawn = propose(eve("approver"));
        assert_eq!(status(decline(&a, &ada, withdrawn, "wrong person")), (200, "was declined by ada: wrong person".into()));

        // A role bound by a proposal, removed before it is approved: stale, saying why.
        a.define_role(role("payer", &["run.decide"]), "ada", None).unwrap();
        let stale = propose(eve("payer"));
        a.remove_role("payer", "ada", None).unwrap();
        let before = kept(&a.access_document());
        assert_eq!(status(approve(&a, &person(&a, "bob"), stale)), (200, "is stale: no role \"payer\"".into()));
        assert_eq!(kept(&a.access_document()), before);

        // Through the run file, the settings change at once, and anything else that gives more
        // waits as it does for anyone; anyone else's change to the settings waits.
        let run_file = Caller { local: Some("bma".into()), ..person(&a, "ada") };
        let more = eve("approver");
        assert!(matches!(govern(&a, &run_file, more.clone(), None, || more.make(&a, "ada", None)).unwrap(), Governed::Waits(_)));
        let off = Change::Approvals { settings: ApprovalSettings { changes: None, ..a.access_document().approvals.clone() } };
        assert!(matches!(govern(&a, &ada, off.clone(), None, || off.make(&a, "ada", None)).unwrap(), Governed::Waits(_)));
        assert!(matches!(govern(&a, &run_file, off.clone(), None, || off.make(&a, "ada", None)).unwrap(), Governed::Made(())));
        assert_eq!(a.access_document().approvals.changes, None);
    }

    /// A new principal is refused a role that would wait, saying to give it after; one that gives
    /// nothing governed, or any while the setting is off, is given at once.
    #[test]
    fn a_new_principal_is_never_made_waiting() {
        let a = universe(Some(1));
        assert!(adding(&a, "fay", Kind::Person, "viewer").is_ok());
        assert!(adding(&a, "fay", Kind::Person, "watch").is_ok());
        assert_eq!(
            adding(&a, "fay", Kind::Person, "approver").unwrap_err(),
            "approver gives run.decide on this host, which waits here until 1 person other than its giver approves it, and a new principal can't wait: invite fay as viewer, then give them approver, which waits"
        );
        // A service never decides an approval, so editor gives it nothing that waits; a role that
        // writes secrets does.
        assert!(adding(&a, "ci", Kind::Service, "editor").is_ok());
        a.define_role(RoleText { kinds: vec![Kind::Service], ..role("keeper", &["secret.set"]) }, "ada", None).unwrap();
        assert!(adding(&a, "ci", Kind::Service, "keeper").unwrap_err().ends_with(": add ci as viewer, then give them keeper, which waits"));
        assert!(adding(&universe(None), "fay", Kind::Person, "admin").is_ok());
    }

    /// Kept in `access.proposals.json` beside the access document, read back whole when the server
    /// starts again, and made from there.
    #[test]
    fn proposals_are_kept_beside_the_access_document() {
        let dir = std::env::temp_dir().join(format!("holon-access-proposals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.json");
        let id = {
            let a = Accounts::open(path.to_str().unwrap()).unwrap();
            for (name, role) in [("ada", "admin"), ("bob", "admin"), ("cy", "viewer")] {
                a.invite(name, role, None, "ada").unwrap();
            }
            a.set_approvals(ApprovalSettings { changes: Some(1), ..ApprovalSettings::default() }, "ada", None).unwrap();
            let change = Change::HostRole { name: "cy".into(), role: "admin".into() };
            let Governed::Waits(p) = govern(&a, &person(&a, "ada"), change.clone(), Some("on call"), || change.make(&a, "ada", None)).unwrap() else { panic!("it waits") };
            p.id
        };
        assert!(dir.join("access.proposals.json").exists());
        let a = Accounts::open(path.to_str().unwrap()).unwrap();
        let p = a.proposals().all().into_iter().find(|p| p.id == id).unwrap();
        assert_eq!((p.author.as_str(), p.what.as_str(), p.why.as_deref(), &p.state), ("ada", "bound admin to cy on this host", Some("on call"), &State::Waiting));
        let made = approve(&a, &person(&a, "bob"), id).unwrap();
        assert!(matches!(made.state, State::Applied { version } if version == a.access_document().version));
        let again = Accounts::open(path.to_str().unwrap()).unwrap();
        assert_eq!(again.proposals().all(), vec![made]);
        assert_eq!(again.host_role("cy"), Some(Role::Admin));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
