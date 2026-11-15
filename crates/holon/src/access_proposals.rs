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
//! No one approves giving more to someone they could act as (D13): who made their way in, a link
//! their password was set from or the email a provider matches them by, or an agent's or a
//! service's owner or one who made a token of it (accounts.rs `acting_as`). A proposal says who
//! could, for its approvers to read, and one made since makes it stale. While giving waits for
//! people, no one makes a way in to someone else who holds what it governs (D12, auth.rs
//! `may_take_over`, with `governing` and `taken_over` here): their name would count as a second
//! person. This machine's run file still does: the machine is the root here, and no account is
//! (D11).
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
    /// `PUT /roles/<name>`: the role whole, as it would be; and, when its author said which version
    /// they read it at (`base`), the role as they read it (null: there was none). Made only while
    /// it is still so (accounts.rs `define_role_as_read`), when it is approved too.
    Role {
        role: RoleText,
        #[serde(default, deserialize_with = "bindings::present", skip_serializing_if = "Option::is_none")]
        read: Option<Option<RoleText>>,
    },
    /// `PUT /groups/<name>`: the group whole, its members all of them; and the group as its author
    /// read it, as for a role.
    Group {
        group: Group,
        #[serde(default, deserialize_with = "bindings::present", skip_serializing_if = "Option::is_none")]
        read: Option<Option<Group>>,
    },
    /// `PATCH /people/<name>` with a role: bound on the host in place of what is bound there.
    HostRole { name: String, role: String },
    /// `PUT /access/approvals`: the settings whole, as they would be.
    Approvals { settings: ApprovalSettings },
    /// `PUT /access/local`: the person this machine's run file signs in as (D45).
    Local { account: String },
}

impl Change {
    /// Make it, by `by`, saying `why`, as its route would.
    pub(crate) fn make(&self, accounts: &Accounts, by: &str, why: Option<&str>) -> Result<(), String> {
        match self.clone() {
            Change::Bind { who, role, scope, expires_at } => accounts.bind(who, &role, scope, expires_at, by, why).map(drop),
            Change::Role { role, read } => accounts.define_role_as_read(role, read, by, why).map(drop),
            Change::Group { group, read } => accounts.set_group_as_read(group, read, by, why).map(drop),
            Change::HostRole { name, role } => accounts.set_role(&name, &role, by, why),
            Change::Approvals { settings } => accounts.set_approvals(settings, by, why).map(drop),
            Change::Local { account } => accounts.set_local(&account, by, why).map(drop),
        }
    }

    /// What its route asks of whoever makes it, on the host (can.rs): asked again of its author
    /// when it is approved.
    fn asks(&self) -> Action {
        match self {
            Change::Role { .. } => Action::RoleDefine,
            Change::HostRole { .. } => Action::PeopleManage,
            Change::Bind { .. } | Change::Group { .. } | Change::Approvals { .. } | Change::Local { .. } => Action::PeopleAssign,
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
    /// Whom it gives more when it is proposed, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) to: Vec<String>,
    /// Who could act as them, in words ("bob made eve's reset link"): its approvers read it, none of
    /// them is one of them, and a way in to one of them made since makes it stale (D13).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) ways_in: Vec<String>,
    /// How many principals had been made here when it was proposed: one it gives more made since
    /// is someone else under the name (accounts.rs `Book::admit`).
    #[serde(default)]
    pub(crate) made: u64,
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
    /// How it came, when not through their own sign-in: this machine's run file, as its OS user
    /// (`local:<user>`, D45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) via: Option<String>,
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
        // Sealed under the server's keys, as the access document is (at_rest.rs).
        let list = match crate::at_rest::read_sealed(&path, crate::at_rest::role::ACCESS_PROPOSALS).map_err(|e| format!("the access proposals {e}"))? {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("the access proposals {path:?}: {e}"))?,
            None => Vec::new(),
        };
        Ok(Proposals { path: Some(path), list: Mutex::new(list) })
    }

    pub(crate) fn memory() -> Proposals {
        Proposals { path: None, list: Mutex::new(Vec::new()) }
    }

    fn save(&self, list: &[Proposal]) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
        crate::at_rest::write_table(path, crate::at_rest::role::ACCESS_PROPOSALS, &text).map_err(|e| format!("the access proposals {path:?}: {e}"))
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

/// Whom `after` ties this machine's run file to that `before` didn't, in words (D45): whatever it
/// decides it then decides as them, taking the steps their roles take, so it gives whoever holds
/// this machine their name and what of theirs waits for people to be given (`gains` from nothing
/// bound to them): "this machine's run file signs in as cy, holding run.decide on this host, as
/// approver".
fn retied(before: &Document, after: &Document, now: u64) -> Vec<String> {
    let Some((to, _)) = retie(before, after) else { return Vec::new() };
    let mut none = after.clone();
    none.bindings.retain(|b| b.who != Subject::Principal(to.clone()));
    for g in &mut none.groups {
        g.members.retain(|m| *m != to);
    }
    let held = gains(&none, after, &|name: &str| (name == to).then_some(Kind::Person), now);
    let held: Vec<String> = grouped(&held).into_iter().map(|(g, actions)| format!("{} {}, as {}", actions.join(", "), g.scope.said(), g.role)).collect();
    match held.is_empty() {
        true => vec![format!("this machine's run file signs in as {to}")],
        false => vec![format!("this machine's run file signs in as {to}, holding {}", held.join("; "))],
    }
}

/// Whom `after` ties this machine's run file to anew, and whom it signs in as before, if it does.
fn retie(before: &Document, after: &Document) -> Option<(String, Option<String>)> {
    after.local.clone().filter(|to| before.local.as_ref() != Some(to)).map(|to| (to, before.local.clone()))
}

/// `gains` in words, one for each principal, role and scope: "run.decide to eve on this host, as
/// approver".
fn said(gains: &[Gain]) -> Vec<String> {
    grouped(gains).into_iter().map(|(g, actions)| format!("{} to {} {}, as {}", actions.join(", "), g.who, g.scope.said(), g.role)).collect()
}

/// `gains` by principal, role and scope, each with its actions' names.
fn grouped(gains: &[Gain]) -> Vec<(&Gain, Vec<&'static str>)> {
    let mut each: Vec<(&Gain, Vec<&str>)> = Vec::new();
    for g in gains {
        match each.iter_mut().find(|(first, _)| first.who == g.who && first.role == g.role && first.scope == g.scope) {
            Some((_, actions)) => actions.push(g.action.name()),
            None => each.push((g, vec![g.action.name()])),
        }
    }
    each
}

/// Whom `gains` gives more, by name, each once.
fn to(gains: &[Gain]) -> Vec<String> {
    let mut names: Vec<String> = gains.iter().map(|g| g.who.clone()).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Who could act as each of `names` besides themselves, with how in words (accounts.rs
/// `acting_as`), as (whom, who, how): what its approvers read, and whom it keeps from approving it
/// (D13).
fn who_could_act(accounts: &Accounts, names: &[String]) -> Vec<(String, String, String)> {
    names.iter().flat_map(|name| accounts.acting_as(name).into_iter().map(move |(who, how)| (name.clone(), who, how))).collect()
}

/// What `name` holds now whose giving waits for people, and that they may use, in words: the first
/// of it, "run.decide on this host, as admin"; None if nothing (the takeover guard, auth.rs: their
/// name would count as another person's, D12).
pub(crate) fn governing(accounts: &Accounts, name: &str) -> Option<String> {
    let kind = accounts.principal(name)?.kind;
    let holds = accounts.holding(name);
    let caller = Caller { name: Some(name.into()), kind: Some(kind), holds: Some(Arc::clone(&holds)), local: None };
    holds.iter().filter(|h| h.role.kinds.contains(&kind)).find_map(|h| {
        let (action, _) = bindings::gives(&h.role, &h.scope).into_iter().find(|(a, on)| governed(*a) && can(&caller, *a, on).is_ok())?;
        Some(format!("{} {}, as {}", action.name(), h.scope.said(), h.role.name))
    })
}

/// What whoever acts as `name` could use whose giving waits for people (`governing`), and whose it
/// is: `name`'s own, or else an agent's or a service's `name` owns, whose tokens they make.
pub(crate) fn governing_as(accounts: &Accounts, name: &str) -> Option<(String, String)> {
    std::iter::once(name.to_string()).chain(accounts.owned(name)).find_map(|who| Some((who.clone(), governing(accounts, &who)?)))
}

/// Why `what` ("resets bob's password") isn't done here but through the run file: `whose` (`name`,
/// or an agent or a service `name` owns) holds `held` (`governing_as`), whose giving waits for `n`
/// people other than its giver, and whoever did it could act as `name`, counted as another person
/// (D12); `instead` says what else to do.
pub(crate) fn taken_over(name: &str, (whose, held): &(String, String), what: &str, n: u8, instead: &str) -> String {
    let holds = if whose == name { format!("{name} holds") } else { format!("{name} owns {whose}, which holds") };
    format!(
        "{holds} {held}, and giving that waits here for {} other than its giver: whoever {what} could act as {name}, counted as another person, so it is done only through this server's machine (its run file); {instead}",
        people(n)
    )
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
    let run_file = caller.local.is_some() && matches!(change, Change::Approvals { .. } | Change::Local { .. });
    if run_file || accounts.access_document().approvals.changes.is_none() {
        return make().map(Governed::Made);
    }
    let by = caller.name.as_deref().unwrap_or("");
    let crate::accounts::Trial { before, after, made, .. } = accounts.trial(|scratch| change.make(scratch, by, why))?;
    let now = epoch_now();
    let kind_of = |name: &str| accounts.principal(name).map(|p| p.kind);
    let grants = gains(&before, &after, &kind_of, now);
    let mut gives = said(&grants);
    gives.extend(loosened(&before.approvals, &after.approvals));
    gives.extend(retied(&before, &after, now));
    let Some(needs) = before.approvals.changes.filter(|_| !gives.is_empty()) else { return make().map(Governed::Made) };
    let id = list.iter().map(|p| p.id).max().unwrap_or(0) + 1;
    let to = to(&grants);
    let ways_in = who_could_act(accounts, &to).into_iter().map(|(_, _, how)| how).collect();
    let p = Proposal { id, author: by.into(), at: now, why: why.map(String::from), change, what: after.what.clone(), gives, to, ways_in, made, base: before.version, needs, approvals: Vec::new(), state: State::Waiting, ended_at: None };
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

/// What a proposal comes to, read again against the document as it is now: the document, what
/// making it gives and to whom, who could act as them, and the accounts' writes when it was read
/// (accounts.rs `unchanged_since`).
struct Read {
    before: Arc<Document>,
    after: Document,
    gained: Vec<Gain>,
    acting: Vec<(String, String, String)>,
    /// Whom it ties this machine's run file to, and whom it signs in as now (`retie`).
    retie: Option<(String, Option<String>)>,
    written: u64,
}

/// `p` read again against the document as it is now, as it is made only as its approvers read it:
/// or why it no longer applies, which makes it stale. Its author is still here, holding what its
/// route asks and what it gives (D19, as when they proposed it); it can still be made; it gives exactly what it said, to
/// exactly whom (a role widened, a group joined or the settings tightened since would make it give
/// more than its approvers read, or other than it); whom it gives more is who it was when it was
/// proposed, not someone removed and added again under the name since; and no way in to them was
/// made since but those its approvers read (D13).
fn read_again(accounts: &Accounts, p: &Proposal, now: u64) -> Result<Read, String> {
    let kind_of = |name: &str| accounts.principal(name).map(|p| p.kind);
    let author = accounts.principal(&p.author).filter(|a| !a.disabled).ok_or_else(|| format!("its author, {}, is no longer here, or is disabled", p.author))?;
    let as_author = Caller { name: Some(author.name.clone()), kind: Some(author.kind), holds: Some(accounts.holding(&author.name)), local: None };
    if can(&as_author, p.change.asks(), &Resource::Host).is_err() {
        return Err(format!("its author, {}, no longer holds {} on this host", p.author, p.change.asks().name()));
    }
    let crate::accounts::Trial { before, after, written, .. } = accounts.trial(|scratch| p.change.make(scratch, &p.author, None))?;
    let gained = gains(&before, &after, &kind_of, now);
    let mut gives = said(&gained);
    gives.extend(loosened(&before.approvals, &after.approvals));
    gives.extend(retied(&before, &after, now));
    let more: Vec<&str> = gives.iter().filter(|g| !p.gives.contains(g)).map(String::as_str).collect();
    let less: Vec<&str> = p.gives.iter().filter(|g| !gives.contains(g)).map(String::as_str).collect();
    match (more.is_empty(), less.is_empty()) {
        (true, true) => {}
        (false, true) => return Err(format!("since it was proposed, it would give {} too", more.join("; "))),
        (true, false) => return Err(format!("since it was proposed, it no longer gives {}", less.join("; "))),
        (false, false) => return Err(format!("since it was proposed, it would give {}, and no longer {}", more.join("; "), less.join("; "))),
    }
    let to = to(&gained);
    if let Some(whom) = to.iter().find(|whom| !p.to.contains(whom)) {
        return Err(format!("since it was proposed, it would give more to {whom} too"));
    }
    if let Some(whom) = to.iter().find(|whom| accounts.principal(whom).is_some_and(|w| w.serial > p.made)) {
        return Err(format!("since it was proposed, {whom} was removed and added again"));
    }
    if let Some(g) = gained.iter().find(|g| can(&as_author, g.action, &bindings::given_on(g.action, &g.scope)).is_err()) {
        return Err(format!("its author, {}, no longer holds {} {}", p.author, g.action.name(), g.scope.said()));
    }
    let retie = retie(&before, &after);
    if let Some((b, a)) = retie.as_ref().and_then(|(to, _)| bindings::beyond(&as_author, &after, Kind::Person, after.reaching(to).map(|(b, _)| b), now, false)) {
        return Err(format!("its author, {}, no longer holds {} {}, which holds {} there", p.author, b.role, b.scope.said(), a.name()));
    }
    let acting = who_could_act(accounts, &to);
    if let Some((_, _, how)) = acting.iter().find(|(whom, _, how)| p.to.contains(whom) && !p.ways_in.contains(how)) {
        return Err(format!("since it was proposed, {how}"));
    }
    Ok(Read { before, after, gained, acting, retie, written })
}

/// Why `caller` (named `name`) may not approve what `read` gives, if they may not: it gives them
/// more (whom it gives more doesn't decide giving it), they don't hold what it gives where it
/// gives it (D6), or they could act as someone it gives more (D13). One that ties this machine's
/// run file to someone gives whoever holds the machine their name and what they hold: it isn't
/// approved through the run file, nor by whom it signs in as now, nor by someone holding less
/// than whom it ties it to (D19).
fn may_not_approve(caller: &Caller, name: &str, id: u64, read: &Read) -> Option<String> {
    if read.gained.iter().any(|g| g.who == name) {
        return Some(format!("access change {id} gives more to you: it is approved by people other than whom it gives more"));
    }
    if let Some(g) = read.gained.iter().find(|g| can(caller, g.action, &bindings::given_on(g.action, &g.scope)).is_err()) {
        return Some(format!("access change {id} gives {} to {} {}, which you don't hold: an access change is approved only by someone who holds what it gives", g.action.name(), g.who, g.scope.said()));
    }
    if let Some((_, _, how)) = read.acting.iter().find(|(_, who, _)| who == name) {
        return Some(format!("access change {id} gives more to someone you could act as ({how}): it is approved by someone who couldn't"));
    }
    let (to, tied) = read.retie.as_ref()?;
    if caller.local.is_some() {
        return Some(format!("access change {id} ties this machine's run file to {to}: it is approved by someone signed in as themself, not through the run file"));
    }
    if tied.as_deref() == Some(name) {
        return Some(format!("access change {id} ties this machine's run file, which signs in as you now, to {to}: it is approved by someone it doesn't sign in as"));
    }
    let (b, a) = bindings::beyond(caller, &read.after, Kind::Person, read.after.reaching(to).map(|(b, _)| b), epoch_now(), false)?;
    Some(format!("access change {id} ties this machine's run file to {to}, who holds {} {}, which holds {} there, which you don't: it is approved only by someone who holds what it gives", b.role, b.scope.said(), a.name()))
}

/// Whether `who`'s approval, given earlier, still counts against `read`: they are still here, a
/// person, enabled, holding people.assign on the host, and may approve what it gives now. One
/// disabled since, or holding less, approved what they no longer could.
fn still_approves(accounts: &Accounts, who: &str, id: u64, read: &Read) -> bool {
    let Some(p) = accounts.principal(who).filter(|p| !p.disabled && p.kind == Kind::Person) else { return false };
    let caller = Caller { name: Some(p.name.clone()), kind: Some(p.kind), holds: Some(accounts.holding(&p.name)), local: None };
    can(&caller, Action::PeopleAssign, &Resource::Host).is_ok() && may_not_approve(&caller, who, id, read).is_none()
}

/// `caller` approves the proposal `id`: it, made if theirs is the last approval it waits for, as
/// its approvers read it, or stale if it no longer applies as they read it (`read_again`). Refused
/// (403) its author, whom it gives more, whoever doesn't hold what it gives where it gives it (D6),
/// read against the document as it is now, and whoever could act as whom it gives more (D13); 409
/// for a second approval by the same person. The approvals given before are read again too, and
/// one that no longer would be given (its approver disabled, or holding less since) doesn't count.
/// Made, it is made only if nothing in the accounts was written since it was read; if something
/// was, it is read again (`TRIES` times, then 503 with nothing changed).
pub(crate) fn approve(accounts: &Accounts, caller: &Caller, id: u64) -> Result<Proposal, Refused> {
    const TRIES: usize = 8;
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
    for _ in 0..TRIES {
        let read = match read_again(accounts, p, now) {
            Ok(read) => read,
            Err(e) => {
                p.state = State::Stale { stale: e };
                p.ended_at = Some(now);
                break;
            }
        };
        if let Some(refused) = may_not_approve(caller, &name, id, &read) {
            return Err((403, refused));
        }
        let mut approvals: Vec<Approval> = p.approvals.iter().filter(|a| still_approves(accounts, &a.who, id, &read)).cloned().collect();
        approvals.push(Approval { who: name.clone(), at: now, via: caller.local.clone() });
        let needs = p.needs.max(read.before.approvals.changes.unwrap_or(0));
        if approvals.len() < usize::from(needs) {
            (p.approvals, p.needs) = (approvals, needs);
            break;
        }
        let who: Vec<&str> = approvals.iter().map(|a| a.who.as_str()).collect();
        let why = format!("{}access change {id}, approved by {}", p.why.as_ref().map(|w| format!("{w}; ")).unwrap_or_default(), and(&who));
        let Some(made) = accounts.unchanged_since(read.written, || p.change.make(accounts, &p.author, Some(&why))) else { continue };
        p.state = match made {
            Ok(()) => State::Applied { version: accounts.access_document().version },
            Err(e) => State::Stale { stale: e },
        };
        (p.approvals, p.needs, p.ended_at) = (approvals, needs, Some(now));
        break;
    }
    if p.state == State::Waiting && p.approvals.iter().all(|a| a.who != name) {
        return Err((503, format!("the accounts were changed each time access change {id} was read to be made: approve it again")));
    }
    let p = p.clone();
    proposals.save(&list).map_err(|e| (500, e))?;
    Ok(p)
}

/// `caller` declines the proposal `id`, saying why: it. Its author may withdraw it so; anyone
/// else declines only what they may approve (`may_not_approve`, read as it is now), so a role that
/// assigns people doesn't turn down whatever gives more than it holds. One that no longer applies
/// is declined by anyone who decides proposals.
pub(crate) fn decline(accounts: &Accounts, caller: &Caller, id: u64, reason: &str) -> Result<Proposal, Refused> {
    let proposals = accounts.proposals();
    let mut list = proposals.list.lock().unwrap();
    let p = list.iter_mut().find(|p| p.id == id).ok_or_else(|| (404, format!("no access change {id}")))?;
    let name = decider(caller, p, "declined")?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err((400, "say why it is declined: {\"reason\": \"…\"}".into()));
    }
    // Anyone else declines only what they may approve (D6, D13), read as it is now: one that no
    // longer applies is declined by anyone who may decide it.
    if p.author != name
        && let Ok(read) = read_again(accounts, p, epoch_now())
        && let Some(refused) = may_not_approve(caller, &name, id, &read)
    {
        return Err((403, format!("{refused}; it is declined only by its author or someone who may approve it")));
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
    /// the group `team`, bound approver on pay; eve a viewer holding `watch` (run.view) on pay; fay
    /// a viewer whose reset link bob made. Each invited by ada. Whoever deployed a version decides
    /// none of its approvals, and `changes` people approve an access change.
    fn universe(changes: Option<u8>) -> Accounts {
        let a = Accounts::memory();
        for (name, role) in [("ada", "admin"), ("bob", "admin"), ("cy", "viewer"), ("dee", "viewer"), ("eve", "viewer"), ("fay", "viewer")] {
            a.invite(name, role, None, "ada").unwrap();
        }
        a.reset("fay", "bob").unwrap();
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
    /// gives more (while the setting is on), and who besides ada holds what it gives and couldn't
    /// act as whom it gives it, of those who may approve it (bob, cy; dee holds no people.assign).
    fn changes(base: &ApprovalSettings) -> Vec<(&'static str, Change, bool, &'static [&'static str])> {
        let bind = |role: &str, scope: Scope| Change::Bind { who: Subject::Principal("eve".into()), role: role.into(), scope, expires_at: None };
        let to_fay = |role: &str| Change::Bind { who: Subject::Principal("fay".into()), role: role.into(), scope: Scope::Host, expires_at: None };
        let team = |members: &[&str]| Change::Group { group: Group { name: "team".into(), about: None, members: members.iter().map(|m| m.to_string()).collect() }, read: None };
        let settings = |f: fn(&mut ApprovalSettings)| {
            let mut s = base.clone();
            f(&mut s);
            Change::Approvals { settings: s }
        };
        vec![
            ("approver bound to eve on the host", bind("approver", Scope::Host), true, &["bob"]),
            ("assigner bound to fay, whose reset link bob made", to_fay("assigner"), true, &["cy"]),
            ("approver bound to fay, whose reset link bob made", to_fay("approver"), true, &[]),
            ("assigner bound to eve on the host", bind("assigner", Scope::Host), true, &["bob", "cy"]),
            ("approver bound to eve until later", Change::Bind { who: Subject::Principal("dee".into()), role: "approver".into(), scope: pay(), expires_at: Some(epoch_now() + 3600) }, false, &[]),
            ("watch, which eve holds on pay, made to decide", Change::Role { role: role("watch", &["run.view", "run.decide"]), read: None }, true, &["bob"]),
            ("eve into team, bound approver on pay", team(&["dee", "eve"]), true, &["bob"]),
            ("eve made an editor", Change::HostRole { name: "eve".into(), role: "editor".into() }, true, &["bob"]),
            ("whoever deployed a version let decide it", settings(|s| s.exclude_deployer = false), true, &["bob", "cy"]),
            ("one fewer approving an access change", settings(|s| s.changes = s.changes.and_then(|n| n.checked_sub(1)).filter(|n| *n > 0)), true, &["bob", "cy"]),
            ("access changes applying at once", settings(|s| s.changes = None), true, &["bob", "cy"]),
            ("one more approving an access change", settings(|s| s.changes = s.changes.map(|n| n + 1)), false, &[]),
            ("agents kept from their steps", settings(|s| s.agents = false), false, &[]),
            ("watch bound to eve on the host", bind("watch", Scope::Host), false, &[]),
            ("dee out of team", team(&[]), false, &[]),
            ("approver narrowed to run.view", Change::Role { role: role("approver", &["run.view"]), read: None }, false, &[]),
        ]
    }

    /// The model (the access plan's 8c and 8e): every kind of change × the setting off, one and two
    /// × every set of the four who might approve it, each in turn. A change that gives no one more
    /// is made at once, as is any while the setting is off. One that does waits, and is made
    /// exactly when N different people other than its author, each a person holding people.assign
    /// on the host and what it gives, and none able to act as whom it gives it, approved it: the
    /// document is then what the change makes of the base, and until then the base. Each refusal is
    /// its rule's: its author, whoever doesn't hold people.assign or what it gives, and whoever
    /// could act as whom it gives it, 403; an approval once it is made, 409.
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

    /// The class (R4): whatever is changed while an access change waits (something else, a role it
    /// binds widened or narrowed, whom it gives more put in a group, the settings tightened, a way
    /// in made to whom it gives more, they removed and invited again, its author or an approver
    /// holding less, an approver disabled, what it gives given already), before any approval or
    /// after the first, approving it makes it exactly as its approvers read it, or not at all.
    /// Made, the document gains just what it says it gives, to just whom, each the principal it was
    /// proposed for, and each approval counted is one that could be given then (a person, enabled,
    /// not its author nor whom it gives more, holding people.assign and what it gives, and unable to
    /// act as whom it gives more); stale, refused or waiting, the document is as it was. Whom it
    /// gives more doesn't approve it, gus's binding lasting longer than his own too (CR:A2).
    #[test]
    fn an_approved_access_change_is_made_as_its_approvers_read_it_whatever_changed_since() {
        fn settings(a: &Accounts, f: impl FnOnce(&mut ApprovalSettings)) {
            let mut s = a.access_document().approvals.clone();
            f(&mut s);
            a.set_approvals(s, "ada", None).unwrap();
        }
        fn to_principal(name: &str) -> Subject {
            Subject::Principal(name.into())
        }
        /// `p`, made from `doc`: the document gained exactly what it says, to exactly whom, each
        /// the principal it was proposed for, and each approval counted could be given on `doc`.
        fn exactly(a: &Accounts, doc: &Document, p: &Proposal, case: &str) {
            let now = epoch_now();
            let after = a.access_document();
            let kind_of = |n: &str| a.principal(n).map(|w| w.kind);
            let gained = gains(doc, &after, &kind_of, now);
            let mut gave = said(&gained);
            gave.extend(loosened(&doc.approvals, &after.approvals));
            gave.extend(retied(doc, &after, now));
            let mut says = p.gives.clone();
            gave.sort();
            says.sort();
            assert_eq!(gave, says, "{case}: it gives what it says");
            assert_eq!(to(&gained), p.to, "{case}: to whom it says");
            for w in &p.to {
                assert!(a.principal(w).unwrap().serial <= p.made, "{case}: {w} is whom it was proposed for");
            }
            assert!(p.approvals.len() >= usize::from(p.needs) && p.needs >= doc.approvals.changes.unwrap_or(0), "{case}: enough approve it");
            let held = doc.index();
            for ap in &p.approvals {
                let w = a.principal(&ap.who).unwrap();
                assert!(w.kind == Kind::Person && !w.disabled && ap.who != p.author && !p.to.contains(&ap.who), "{case}: {} approves it", ap.who);
                let then = bindings::as_caller(held.get(&ap.who).map(Vec::as_slice).unwrap_or_default(), &ap.who, Kind::Person, now, Some(now));
                assert!(can(&then, Action::PeopleAssign, &Resource::Host).is_ok(), "{case}: {} holds people.assign", ap.who);
                for g in &gained {
                    assert!(can(&then, g.action, &bindings::given_on(g.action, &g.scope)).is_ok(), "{case}: {} holds {}", ap.who, g.action.name());
                }
                assert!(!p.to.iter().any(|w| a.acting_as(w).iter().any(|(by, _)| *by == ap.who)), "{case}: {} could act as whom it gives more", ap.who);
                if let Some((to, tied)) = retie(doc, &after) {
                    assert_ne!(tied.as_deref(), Some(ap.who.as_str()), "{case}: the run file signs in as {}", ap.who);
                    assert!(bindings::beyond(&then, doc, Kind::Person, doc.reaching(&to).map(|(b, _)| b), now, false).is_none(), "{case}: {} holds no less than {to}", ap.who);
                }
            }
        }
        /// Something changed while it waits, by name.
        type Meanwhile = (&'static str, fn(&Accounts));
        /// (changes, what it is, what changed meanwhile, after the first approval) to what became
        /// of it and who approved it.
        type Outcomes<'a> = std::collections::HashMap<(u8, &'a str, &'a str, bool), (String, Vec<String>)>;
        let later = epoch_now() + 3 * 86_400;
        let events: [Meanwhile; 13] = [
            ("nothing", |_| {}),
            ("watch bound to fay on pay", |a| {
                a.bind(to_principal("fay"), "watch", pay(), None, "ada", None).unwrap();
            }),
            ("approver widened to people.assign", |a| {
                a.define_role(role("approver", &["run.decide", "run.view", "people.assign"]), "ada", None).unwrap();
            }),
            ("approver narrowed to run.view", |a| {
                a.define_role(role("approver", &["run.view"]), "ada", None).unwrap();
            }),
            ("eve into team", |a| {
                a.set_group(Group { name: "team".into(), about: None, members: vec!["dee".into(), "eve".into()] }, "ada", None).unwrap();
            }),
            ("one more approving an access change", |a| settings(a, |s| s.changes = s.changes.map(|n| n + 1))),
            ("agents kept from their steps", |a| settings(a, |s| s.agents = false)),
            ("cy made eve's reset link", |a| {
                a.reset("eve", "cy").unwrap();
            }),
            ("eve and fay removed and invited again", |a| {
                for name in ["eve", "fay"] {
                    a.remove(name, "ada").unwrap();
                    a.invite(name, "viewer", None, "ada").unwrap();
                }
            }),
            ("ada made a viewer", |a| a.set_role("ada", "viewer", "bob", None).unwrap()),
            ("assigner without people.assign", |a| {
                a.define_role(role("assigner", &["people.view", "role.view", "role.define"]), "ada", None).unwrap();
            }),
            ("bob disabled", |a| a.set_disabled("bob", true).unwrap()),
            ("admin bound to gus for good", |a| {
                a.bind(to_principal("gus"), "admin", Scope::Host, None, "ada", None).unwrap();
            }),
        ];
        let mut outcomes: Outcomes<'_> = Outcomes::new();
        for setting in [1u8, 2] {
            let base = ApprovalSettings { exclude_deployer: true, changes: Some(setting), ..ApprovalSettings::default() };
            let mut kinds: Vec<(&str, Change)> = changes(&base).into_iter().filter(|(_, _, gives, _)| *gives).map(|(what, change, ..)| (what, change)).collect();
            kinds.push(("admin to gus for good, who holds it until later", Change::Bind { who: to_principal("gus"), role: "admin".into(), scope: Scope::Host, expires_at: None }));
            kinds.push(("this machine's run file tied to dee", Change::Local { account: "dee".into() }));
            for (what, change) in &kinds {
                for (event, meanwhile) in events {
                    for after_first in [false, true] {
                        let case = format!("{what}, changes {setting}, {event} {}", if after_first { "after the first approval" } else { "before any" });
                        let a = universe(Some(setting));
                        a.invite("gus", "viewer", None, "ada").unwrap();
                        a.bind(to_principal("gus"), "admin", Scope::Host, Some(later), "ada", None).unwrap();
                        let Governed::Waits(p) = govern(&a, &person(&a, "ada"), change.clone(), None, || change.make(&a, "ada", None)).unwrap() else { panic!("{case}: it waits") };
                        if !after_first {
                            meanwhile(&a);
                        }
                        let mut pending = after_first;
                        let mut last = (*p).clone();
                        for who in ["bob", "gus", "cy", "dee"] {
                            if a.principal(who).is_none_or(|w| w.disabled) {
                                continue;
                            }
                            let doc = a.access_document();
                            let answer = approve(&a, &person(&a, who), p.id);
                            let now = a.proposals().all().into_iter().find(|q| q.id == p.id).unwrap();
                            if p.to.iter().any(|w| w == who) && !matches!(now.state, State::Stale { .. }) {
                                assert_eq!(answer.as_ref().err().map(|(s, _)| *s), Some(403), "{case}: {who} is given more: {answer:?}");
                            }
                            if matches!(now.state, State::Applied { .. }) {
                                exactly(&a, &doc, &now, &case);
                            } else {
                                assert_eq!(kept(&a.access_document()), kept(&doc), "{case}: {who}: nothing changes until it is made");
                            }
                            last = now;
                            if last.state != State::Waiting {
                                break;
                            }
                            if pending && answer.is_ok() {
                                meanwhile(&a);
                                pending = false;
                            }
                        }
                        let state = match &last.state {
                            State::Applied { .. } => "made".to_string(),
                            State::Waiting => "waits".to_string(),
                            State::Stale { stale } => format!("stale: {stale}"),
                            State::Declined { .. } => unreachable!("no one declines it here"),
                        };
                        outcomes.insert((setting, what, event, after_first), (state, last.approvals.iter().map(|a| a.who.clone()).collect()));
                    }
                }
            }
        }
        let outcome = |setting: u8, what: &str, event: &str, after_first: bool| outcomes[&(setting, what, event, after_first)].clone();
        let state = |setting: u8, what: &str, event: &str| outcome(setting, what, event, false).0;
        let (eve, gus) = ("approver bound to eve on the host", "admin to gus for good, who holds it until later");
        let made_by = |who: &[&str]| ("made".to_string(), who.iter().map(|w| w.to_string()).collect::<Vec<_>>());
        assert_eq!(outcome(1, eve, "nothing", false), made_by(&["bob"]));
        assert_eq!(outcome(1, eve, "watch bound to fay on pay", false), made_by(&["bob"]));
        assert_eq!(outcome(1, eve, "one more approving an access change", false), made_by(&["bob", "gus"]));
        let widened = state(1, eve, "approver widened to people.assign");
        assert!(widened.starts_with("stale: since it was proposed, it would give run.decide, people.assign to eve on this host, as approver"), "{widened}");
        assert_eq!(state(1, eve, "approver narrowed to run.view"), "stale: since it was proposed, it no longer gives run.decide to eve on this host, as approver");
        assert_eq!(state(1, eve, "cy made eve's reset link"), "stale: since it was proposed, cy made eve's reset link");
        assert_eq!(state(1, eve, "eve and fay removed and invited again"), "stale: since it was proposed, eve was removed and added again");
        assert_eq!(state(1, eve, "ada made a viewer"), "stale: its author, ada, no longer holds people.assign on this host");
        let undone = state(1, "whoever deployed a version let decide it", "agents kept from their steps");
        assert!(undone.starts_with("stale: since it was proposed, it would give"), "{undone}");
        let given = state(1, gus, "admin bound to gus for good");
        assert!(given.starts_with("stale: since it was proposed, it no longer gives"), "{given}");
        let tied = "this machine's run file tied to dee";
        assert_eq!(outcome(1, tied, "nothing", false), made_by(&["bob"]));
        assert_eq!(outcome(2, tied, "nothing", false), made_by(&["bob", "gus"]));
        let widened = state(1, tied, "approver widened to people.assign");
        assert!(widened.starts_with("stale: since it was proposed, it would give this machine's run file signs in as dee, holding run.decide, people.assign on this host"), "{widened}");
        // An earlier approval from someone disabled since doesn't count; gus, given more, never
        // approves what gives him more.
        assert_eq!(outcome(2, eve, "bob disabled", true), ("waits".into(), vec!["gus".into()]));
        assert_eq!(outcome(2, gus, "nothing", false), ("waits".into(), vec!["bob".into()]));
    }

    /// What `approve` makes is made only if nothing in the accounts was written since it was read
    /// (accounts.rs `unchanged_since`): with a write between, nothing is made, and read again it
    /// is. Only its first write is asked: a second, after its own, is made as any is.
    #[test]
    fn an_access_change_is_not_made_over_a_write_since_it_was_read() {
        let a = universe(Some(1));
        let change = Change::Bind { who: Subject::Principal("eve".into()), role: "approver".into(), scope: Scope::Host, expires_at: None };
        let read = a.trial(|scratch| change.make(scratch, "ada", None)).unwrap();
        a.reset("dee", "bob").unwrap();
        let before = kept(&a.access_document());
        assert_eq!(a.unchanged_since(read.written, || change.make(&a, "ada", None)), None);
        assert_eq!(kept(&a.access_document()), before);
        let again = a.trial(|scratch| change.make(scratch, "ada", None)).unwrap();
        let made = a.unchanged_since(again.written, || {
            change.make(&a, "ada", None)?;
            a.reset("dee", "cy").map(drop)
        });
        assert_eq!(made, Some(Ok(())));
        assert_ne!(kept(&a.access_document()), before);
        assert_eq!(a.acting_as("dee"), [("cy".to_string(), "cy made dee's reset link".to_string())]);
    }

    /// Tying this machine's run file to someone (D45) gives whoever holds the machine their name
    /// and what they hold: it waits for people, saying so, and isn't approved through the run file,
    /// nor by whom it signs in as now, nor by someone holding less than whom it ties it to (D19).
    /// Through the run file itself, the machine's root (D11), it is made at once. An approval
    /// through the run file says so (`via`).
    #[test]
    fn tying_the_run_file_to_someone_waits_for_people() {
        let a = universe(Some(1));
        a.invite("gus", "admin", None, "ada").unwrap();
        a.set_local("bob", "ada", None).unwrap();
        let tie = |to: &str| Change::Local { account: to.into() };
        let Governed::Waits(p) = govern(&a, &person(&a, "ada"), tie("dee"), None, || tie("dee").make(&a, "ada", None)).unwrap() else { panic!("it waits") };
        assert_eq!(p.gives.len(), 1);
        assert!(p.gives[0].starts_with("this machine's run file signs in as dee, holding run.decide on this host, as approver; run.decide "), "{:?}", p.gives);
        let run_file = Caller { local: Some("bma".into()), ..person(&a, "gus") };
        let refused = |caller: &Caller| approve(&a, caller, p.id).map(|p| p.state.said());
        assert_eq!(refused(&run_file), Err((403, format!("access change {} ties this machine's run file to dee: it is approved by someone signed in as themself, not through the run file", p.id))));
        assert_eq!(refused(&person(&a, "bob")), Err((403, format!("access change {} ties this machine's run file, which signs in as you now, to dee: it is approved by someone it doesn't sign in as", p.id))));
        let (code, said) = refused(&person(&a, "cy")).unwrap_err();
        assert!(code == 403 && said.contains("which you don't"), "{said}");
        assert_eq!(a.access_document().local.as_deref(), Some("bob"), "nothing made yet");
        assert_eq!(refused(&person(&a, "gus")), Ok(format!("was made, as access version {}", a.access_document().version)));
        assert_eq!(a.access_document().local.as_deref(), Some("dee"));
        // Through the run file, at once.
        assert!(matches!(govern(&a, &run_file, tie("eve"), None, || tie("eve").make(&a, "gus", None)).unwrap(), Governed::Made(())));
        assert_eq!(a.access_document().local.as_deref(), Some("eve"));
        // Another change, approved through the run file, says so.
        let editor = Change::HostRole { name: "fay".into(), role: "editor".into() };
        let Governed::Waits(p) = govern(&a, &person(&a, "ada"), editor.clone(), None, || editor.make(&a, "ada", None)).unwrap() else { panic!("it waits") };
        let made = approve(&a, &run_file, p.id).unwrap();
        assert_eq!(made.approvals.iter().map(|a| (a.who.as_str(), a.via.as_deref())).collect::<Vec<_>>(), [("gus", Some("bma"))]);
    }

    /// Whoever could act as someone a change gives more (D13) doesn't approve it, and the proposal
    /// says who could: whoever set a person's email or made their link, and an agent's owner. A way
    /// in made since it was proposed makes it stale, the document unchanged. What someone holds
    /// that waits for people to be given is said for the takeover guard (`governing`).
    #[test]
    fn whoever_could_act_as_whom_a_change_gives_more_doesnt_approve_it() {
        let a = universe(Some(1));
        let ada = person(&a, "ada");
        let propose = |who: &str, role: &str| {
            let change = Change::Bind { who: Subject::Principal(who.into()), role: role.into(), scope: Scope::Host, expires_at: None };
            match govern(&a, &ada, change.clone(), None, || change.make(&a, "ada", None)).unwrap() {
                Governed::Waits(p) => *p,
                Governed::Made(()) => panic!("{change:?} waits"),
            }
        };
        let status = |r: Result<Proposal, Refused>| r.map_or_else(|(s, e)| (s, e), |p| (200, p.state.said()));
        a.set_email("eve", Some("eve@example.com".into()), "cy").unwrap();
        let p = propose("eve", "assigner");
        assert_eq!((p.to.as_slice(), p.ways_in.as_slice()), (&["eve".to_string()][..], &["ada made eve's invite link".to_string(), "cy set eve's email".to_string()][..]));
        assert_eq!(
            status(approve(&a, &person(&a, "cy"), p.id)),
            (403, format!("access change {} gives more to someone you could act as (cy set eve's email): it is approved by someone who couldn't", p.id))
        );
        assert_eq!(status(approve(&a, &person(&a, "bob"), p.id)).1, format!("was made, as access version {}", a.access_document().version));

        // dee's way in, made again since: stale, saying who made it.
        let p = propose("dee", "assigner");
        a.reset("dee", "bob").unwrap();
        let before = kept(&a.access_document());
        assert_eq!(status(approve(&a, &person(&a, "cy"), p.id)), (200, "is stale: since it was proposed, bob made dee's reset link".into()));
        assert_eq!(kept(&a.access_document()), before);

        // An agent: its owner could act as it, and whoever could act as its owner, who makes its
        // tokens (the audit's R19).
        a.define_role(RoleText { kinds: vec![Kind::Agent], ..role("classifier", &["run.decide", "run.view"]) }, "ada", None).unwrap();
        a.add("bot", Kind::Agent, "viewer", "bob", "ada").unwrap();
        a.invite("gus", "admin", None, "ada").unwrap();
        a.reset("bob", "gus").unwrap();
        let p = propose("bot", "classifier");
        assert_eq!(p.ways_in, ["bob owns bot", "gus made bob's reset link, and bob owns bot"]);
        assert!(status(approve(&a, &person(&a, "bob"), p.id)).1.contains("(bob owns bot)"));
        assert!(status(approve(&a, &person(&a, "gus"), p.id)).1.contains("(gus made bob's reset link, and bob owns bot)"));

        assert_eq!(governing(&a, "dee").as_deref(), Some("run.decide on this host, as approver"));
        assert_eq!(governing(&a, "eve").as_deref(), Some("people.view on this host, as assigner"), "given just now");
        assert_eq!(governing(&a, "fay"), None, "a viewer");
        assert_eq!(governing(&a, "nobody"), None);
        a.add("ci", Kind::Service, "editor", "ada", "ada").unwrap();
        assert_eq!(governing(&a, "ci"), None, "an editor's run.decide, which a service never uses");
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
