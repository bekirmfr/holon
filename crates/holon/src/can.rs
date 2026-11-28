//! Who may do what (docs/L4s/access-and-approvals-L4.md §4, §5.1 and §7): the catalogue of what
//! may be done, the built-in roles made from it, `can`, which answers whether a caller may do an
//! action to a resource, as the roles bound to it there hold it (bindings.rs), and the table of
//! every route the control plane answers, each with what it asks of `can`
//! (docs/L4s/can-and-catalogue-plan.md).
//!
//! The gate (access.rs) finds a request's route here. A path no route has is answered 404 once its
//! caller is known, so no one learns which paths there are without signing in; a route whose rule
//! the caller doesn't meet is refused 403, in the words below. A handler asks `can` again once it
//! knows more: whose saved flow it is (flows.rs), which flow a deploy or a proposal is of, and
//! whether its caller approves what a deploy needs (deploy.rs, proposals.rs), whether a flow lets
//! agents run an action (deploy.rs) or read its log lines (logs.rs), and which approval point is
//! decided (durable.rs). A list asks about every flow at once, and keeps the flows its caller may
//! view (flows.rs, deploy.rs, proposals.rs, summary.rs, activity.rs).
//!
//! The built-in roles:
//!
//! - `viewer`: reads flows, deployments, runs and their state, a secret's names and history, and
//!   the host's policy, and exports runs as evidence;
//! - `agent`: what a viewer does, and proposing: storing units, previewing and deploying a flow
//!   that needs no approval it hasn't had (what does is kept as a proposal, proposals.rs), saving a
//!   flow that isn't someone else's, calling a deployed one, and a restore's dry run. It never
//!   approves, grants, starts, stops, deletes or writes a secret: those are a person's
//!   (docs/L4s/agent-surface-L4.md);
//! - `editor`: what a flow's author and its operator do: saving, deploying and approving what a
//!   deploy needs, restoring, running, pausing and stopping, deleting, calling, deciding an
//!   approval, reloading the policy and checking the run logs again;
//! - `admin`: everything, a flow's secrets, the host's resources and pages (`/admin/…`) and people
//!   (`/people…`) included.
//!
//! A viewer is anyone; an agent is an agent; an editor a person or a service; an admin a person.
//! An admin defines roles of their own (bindings.rs's `RoleText`), of the same shape: what they
//! allow, from the catalogue, and which kinds may hold them. Past the roles, the host's fixed
//! rules, which no role overrides: a service never decides an approval, and an agent takes only
//! a step written for an agent; an agent never does what is a person's (a person's step of an
//! approval, approving a deploy, starting, stopping, deleting or restoring a flow, writing a
//! secret, the host's people, roles, resources and operations), runs a flow's action, and reads
//! its log lines, only where the flow lets agents; and a person manages the tokens of an agent or
//! a service they own, as an admin may for any.

use std::sync::{Arc, LazyLock};

use rouille::Response;

use crate::access::{Caller, Role};
use crate::accounts::{Accounts, Kind};
use crate::bindings::Held;
use crate::http::err;
use Action::*;
use Holds::{Do, For, Own};

/// What may be done, to what (the design's §4). Fixed here: roles are made from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    FlowView,
    FlowEdit,
    FlowPropose,
    FlowDeploy,
    FlowApproveDeploy,
    FlowRestore,
    FlowRestoreDryRun,
    FlowLifecycle,
    FlowDelete,
    RunView,
    RunSteps,
    RunCall,
    RunDecide,
    RunExport,
    StateView,
    StateWrite,
    SecretList,
    SecretSet,
    SecretRollBack,
    ResourceView,
    ResourceBind,
    ResourceUnbind,
    PeopleView,
    PeopleManage,
    PeopleAssign,
    RoleView,
    RoleDefine,
    PolicyView,
    PolicyEdit,
    HostView,
    HostCheck,
    HostOperate,
}

impl Action {
    /// The whole catalogue: what a binding of a role gives (bindings.rs), and what the tests hold
    /// to the design.
    pub(crate) const ALL: [Action; 32] = [
        FlowView, FlowEdit, FlowPropose, FlowDeploy, FlowApproveDeploy, FlowRestore, FlowRestoreDryRun, FlowLifecycle, FlowDelete,
        RunView, RunSteps, RunCall, RunDecide, RunExport,
        StateView, StateWrite,
        SecretList, SecretSet, SecretRollBack,
        ResourceView, ResourceBind, ResourceUnbind,
        PeopleView, PeopleManage, PeopleAssign,
        RoleView, RoleDefine,
        PolicyView, PolicyEdit,
        HostView, HostCheck, HostOperate,
    ];

    /// As the design and the refusals write it: `flow.approve-deploy`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            FlowView => "flow.view",
            FlowEdit => "flow.edit",
            FlowPropose => "flow.propose",
            FlowDeploy => "flow.deploy",
            FlowApproveDeploy => "flow.approve-deploy",
            FlowRestore => "flow.restore",
            FlowRestoreDryRun => "flow.restore-dry-run",
            FlowLifecycle => "flow.lifecycle",
            FlowDelete => "flow.delete",
            RunView => "run.view",
            RunSteps => "run.steps",
            RunCall => "run.call",
            RunDecide => "run.decide",
            RunExport => "run.export",
            StateView => "state.view",
            StateWrite => "state.write",
            SecretList => "secret.list",
            SecretSet => "secret.set",
            SecretRollBack => "secret.roll-back",
            ResourceView => "resource.view",
            ResourceBind => "resource.bind",
            ResourceUnbind => "resource.unbind",
            PeopleView => "people.view",
            PeopleManage => "people.manage",
            PeopleAssign => "people.assign",
            RoleView => "role.view",
            RoleDefine => "role.define",
            PolicyView => "policy.view",
            PolicyEdit => "policy.edit",
            HostView => "host.view",
            HostCheck => "host.check",
            HostOperate => "host.operate",
        }
    }

    /// The action the catalogue names `name`, if it names one.
    pub(crate) fn named(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// What an action is done to. Its slugs and names are what a binding's scope is checked against
/// (bindings.rs's `Held::covers`, the design's §5.3).
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum Resource<'a> {
    /// The host: its policy, its resources and pages, its people.
    Host,
    /// Every flow at once (a list), or one not named yet (a unit stored, a flow saved anew).
    Flows,
    /// A flow by its slug, saved or deployed, with its runs, its state and its secrets.
    Flow(&'a str),
    /// Every flow deployed with a tag, now and later: what a binding on the tag gives.
    Tag(&'a str),
    /// A flow as a version would make it, with the version's tags: a deploy asks about it beside
    /// the flow as it is deployed now (the plan's D22).
    Version { slug: &'a str, tags: &'a [String] },
    /// A saved flow, and who saved it (none when no one was named, as on a server without accounts).
    Saved { name: &'a str, by: Option<&'a str> },
    /// A deployment's action, and whether its grant lets agents run it (`AgentRuns`).
    Action { slug: &'a str, name: &'a str, agents_may: bool },
    /// A deployment's log lines, and whether its grant lets agents read them (`AgentReadsLogs`).
    Logs { slug: &'a str, agents_may: bool },
    /// A run of `slug` waiting at the approval point `name`: none for the decide route's
    /// question, before its body names the point, which is some approval of the flow.
    Approval { slug: &'a str, name: Option<&'a str> },
    /// A principal, and the person who owns it.
    Principal { name: &'a str, owner: Option<&'a str> },
}

impl Resource<'_> {
    /// Whether `action` is one done to this kind of resource (the design's §7, its second check).
    pub(crate) fn takes(&self, action: Action) -> bool {
        let of = action.name().split('.').next().unwrap_or_default();
        match self {
            Resource::Host => matches!(of, "resource" | "people" | "role" | "policy" | "host"),
            Resource::Flows => matches!(of, "flow" | "run"),
            Resource::Flow(_) | Resource::Tag(_) => matches!(of, "flow" | "run" | "state" | "secret"),
            Resource::Version { .. } => of == "flow",
            Resource::Saved { .. } => action == FlowEdit,
            Resource::Action { .. } => action == RunCall,
            Resource::Logs { .. } => action == RunView,
            Resource::Approval { .. } => action == RunDecide,
            Resource::Principal { .. } => action == PeopleManage,
        }
    }
}

/// What a role holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Holds {
    /// Every action.
    All,
    Do(Action),
    /// An action only on what is its holder's own: a flow it saved, or one not saved yet.
    Own(Action),
    /// An action only for these kinds of its holders: which kinds a role's run.steps reaches is
    /// one line of the table (R55).
    For(Action, &'static [Kind]),
}

/// A role, built-in or an admin's own: what it holds, and which kinds of principal may hold it.
#[derive(Debug, PartialEq)]
pub(crate) struct RoleDef {
    pub(crate) name: String,
    /// The built-in role it is, if it is one.
    pub(crate) builtin: Option<Role>,
    holds: Vec<Holds>,
    pub(crate) kinds: Vec<Kind>,
}

impl RoleDef {
    /// An admin's role, `name`: it allows `allows`, to `kinds`; the same role whatever order they
    /// are written in.
    pub(crate) fn custom(name: &str, allows: &[Action], kinds: &[Kind]) -> RoleDef {
        let holds = Action::ALL.into_iter().filter(|a| allows.contains(a)).map(Do).collect();
        RoleDef { name: name.into(), builtin: None, holds, kinds: ANYONE.iter().copied().filter(|k| kinds.contains(k)).collect() }
    }

    /// Each action it holds on something, in the catalogue's order.
    pub(crate) fn allows(&self) -> Vec<Action> {
        Action::ALL.into_iter().filter(|a| self.holds_any(*a)).collect()
    }

    /// Whether it holds `action` on `on`, for `who`.
    fn holds(&self, action: Action, on: &Resource, who: &Caller) -> bool {
        self.holds.iter().any(|h| match *h {
            Holds::All => true,
            Do(a) => a == action,
            For(a, kinds) => a == action && who.kind.is_some_and(|k| kinds.contains(&k)),
            Own(a) => {
                a == action
                    && match on {
                        Resource::Flows => true,
                        Resource::Saved { by, .. } => by.is_some() && *by == who.name.as_deref(),
                        _ => false,
                    }
            }
        })
    }

    /// Whether it holds `action` only on what is its holder's own.
    fn holds_own(&self, action: Action) -> bool {
        self.holds.iter().any(|h| matches!(*h, Own(a) if a == action))
    }

    /// Whether it holds `action` on anything.
    pub(crate) fn holds_any(&self, action: Action) -> bool {
        self.holds.iter().any(|h| matches!(*h, Holds::All) || matches!(*h, Do(a) | Own(a) | For(a, _) if a == action))
    }

    /// Whether, bound to a principal of `kind` where a flow is, it holds `action` there.
    pub(crate) fn holds_for(&self, action: Action, kind: Kind) -> bool {
        self.kinds.contains(&kind)
            && self.holds.iter().any(|h| match *h {
                Holds::All => true,
                Do(a) => a == action,
                For(a, kinds) => a == action && kinds.contains(&kind),
                Own(_) => false,
            })
    }
}

const ANYONE: &[Kind] = &[Kind::Person, Kind::Agent, Kind::Service];

/// The built-in roles (the design's §5.1): today's, as data.
pub(crate) static BUILTIN: LazyLock<[Arc<RoleDef>; 4]> = LazyLock::new(|| {
    let def = |role: Role, holds: &[Holds], kinds: &[Kind]| Arc::new(RoleDef { name: role.name().into(), builtin: Some(role), holds: holds.to_vec(), kinds: kinds.to_vec() });
    [
        // A run's steps and values, to every kind the role is bound to: leaving a kind out here is
        // what holding it like an agent would be (R55).
        def(Role::Viewer, &[Do(FlowView), Do(RunView), For(RunSteps, ANYONE), Do(RunExport), Do(StateView), Do(SecretList), Do(PolicyView)], ANYONE),
        // A viewer's but a run's steps, which a person gives it where they bind one that holds them.
        def(
            Role::Agent,
            &[
                Do(FlowView), Do(RunView), Do(RunExport), Do(StateView), Do(SecretList), Do(PolicyView),
                Own(FlowEdit), Do(FlowPropose), Do(FlowRestoreDryRun), Do(RunCall),
            ],
            &[Kind::Agent],
        ),
        def(
            Role::Editor,
            &[
                Do(FlowView), Do(RunView), For(RunSteps, ANYONE), Do(RunExport), Do(StateView), Do(SecretList), Do(PolicyView),
                Do(FlowEdit), Do(FlowPropose), Do(FlowDeploy), Do(FlowApproveDeploy), Do(FlowRestore), Do(FlowRestoreDryRun), Do(FlowLifecycle), Do(FlowDelete),
                Do(RunCall), Do(RunDecide), Do(StateWrite), Do(PolicyEdit), Do(HostCheck),
            ],
            &[Kind::Person, Kind::Service],
        ),
        def(Role::Admin, &[Holds::All], &[Kind::Person]),
    ]
});

/// The built-in role `role` names.
pub(crate) fn role(role: Role) -> &'static Arc<RoleDef> {
    match role {
        Role::Viewer => &BUILTIN[0],
        Role::Agent => &BUILTIN[1],
        Role::Editor => &BUILTIN[2],
        Role::Admin => &BUILTIN[3],
    }
}

/// The built-in role named `name`, if one is.
pub(crate) fn builtin(name: &str) -> Option<&'static Arc<RoleDef>> {
    BUILTIN.iter().find(|d| d.name == name)
}

/// Why a caller may not: what the 403 says.
#[derive(Debug, PartialEq)]
pub(crate) struct Refusal(pub(crate) String);

impl Refusal {
    pub(crate) fn response(&self) -> Response {
        err(403, self.0.clone())
    }
}

const ADMINS: &str = "that needs an admin";
const AGENTS: &str = "an agent proposes and calls; approving, granting, starting, stopping and deleting are a person's";
const VIEWERS: &str = "a viewer only reads";

/// Whether `who` may do `action` to `on`, or why not (the design's §7). Authentication has already
/// refused a principal that is disabled or unknown, and no token narrows what it may yet (§5.4). So:
/// some role bound to the caller where `on` is holds the action, for the caller's kind, or the
/// caller owns it (a person, the tokens of what they own); and the host's fixed rules don't forbid
/// it. A caller holding nothing at all (`holds` none) is on a server without accounts, and may do
/// everything; one holding no role may do nothing a role holds.
pub(crate) fn can(who: &Caller, action: Action, on: &Resource) -> Result<(), Refusal> {
    debug_assert!(on.takes(action), "{} isn't done to {on:?}", action.name());
    debug_assert!(who.holds.is_some() || who.name.is_none(), "only a server without accounts has a caller who holds nothing at all");
    let held = match &who.holds {
        None => true,
        Some(held) => covering(held, on).any(|def| who.kind.is_some_and(|k| def.kinds.contains(&k)) && def.holds(action, on, who)),
    };
    if !held && !owns(who, action, on) {
        return Err(refused(who, action, on));
    }
    fixed(who, action, on)
}

/// The binding `who` is bound to the role named `role` through, directly or through a group,
/// where `on` is, for its kind: what an approval's step with a role asks of whoever fills it (the
/// design's §8.3, the plan's D37). By name: an admin holding everything doesn't hold `reviewer`.
/// No role named, a role that holds run.decide there. Only a binding of the document: the run
/// file's admin, which none holds, never decides (D45). A decision keeps it as what let it (D43).
pub(crate) fn holding<'a>(who: &'a Caller, role: Option<&str>, on: &Resource) -> Option<&'a Held> {
    let kind = who.kind?;
    who.holds.as_deref()?.iter().filter(|h| h.from.is_some() && h.role.kinds.contains(&kind) && h.covers(on)).find(|h| match role {
        Some(role) => h.role.name == role,
        None => h.role.holds(RunDecide, on, who),
    })
}

/// Whether `who` is shown where the host's resources are: a folder's path, a database's, a mail
/// server's, a feed's and a queue's address, and the sources their URLs are read from. Whoever may
/// bind one (web console L4, Q4); everyone else sees what each does, not where it is (audit R51).
pub(crate) fn sees_where(who: &Caller) -> bool {
    can(who, ResourceBind, &Resource::Host).is_ok()
}

/// Whether `who` is shown where the server keeps its data: whoever views the host, whose page says
/// so (`/admin/host`).
pub(crate) fn sees_data(who: &Caller) -> bool {
    can(who, HostView, &Resource::Host).is_ok()
}

/// Whether `who` may do `action` to every flow, those no one has named yet included: as a binding
/// on the host holds it. No binding names the empty slug (bindings.rs), so only the host covers it.
pub(crate) fn everywhere(who: &Caller, action: Action) -> bool {
    can(who, action, &Resource::Flow("")).is_ok()
}

/// The owner's rule: a person manages the tokens of an agent or a service they own (the OS user as
/// `local:<user>`, while its run file is tied to no account).
fn owns(who: &Caller, action: Action, on: &Resource) -> bool {
    let Resource::Principal { owner: Some(owner), .. } = on else { return false };
    action == PeopleManage && who.kind == Some(Kind::Person) && who.name.as_deref() == Some(*owner)
}

/// What is a person's, whatever role an agent holds (the plan's D20): approving a deploy, starting,
/// stopping, deleting and restoring a flow (its dry run is an agent's), writing a secret, and the
/// host's people, roles, resources and operations.
fn a_persons(action: Action) -> bool {
    let of = action.name().split('.').next().unwrap_or_default();
    matches!(action, FlowApproveDeploy | FlowLifecycle | FlowDelete | FlowRestore | SecretSet | SecretRollBack | HostOperate) || matches!(of, "people" | "role" | "resource")
}

/// The host's fixed rules (the design's §8.4), past what a role holds.
fn fixed(who: &Caller, action: Action, on: &Resource) -> Result<(), Refusal> {
    // An agent by its kind, whatever role it holds.
    let agent = who.kind == Some(Kind::Agent);
    match on {
        // A person or an agent decides, each the steps of its kind (durable.rs): not a service,
        // not the nameless caller of a server without accounts.
        _ if action == RunDecide => match (who.name.as_deref(), who.kind) {
            (None, _) => Err(Refusal("deciding an approval needs a person signed in to Holon, and this server has no accounts".into())),
            (Some(name), Some(Kind::Service)) => Err(Refusal(format!("{name} is a service, a script, not a person: sign in to decide an approval"))),
            _ => Ok(()),
        },
        // No role an admin defines gives an agent what is a person's.
        _ if agent && a_persons(action) => Err(Refusal(format!("{} may not do that: {AGENTS}", who.name.as_deref().unwrap_or_default()))),
        // The flow lets agents, as a person granted it.
        Resource::Action { slug, name, agents_may: false } if agent => Err(Refusal(format!(
            "agents may not run {slug:?}'s action {name:?}: a person lets them, under \"agents may run\" in its deploy settings, and approves it"
        ))),
        Resource::Logs { slug, agents_may: false } if agent => {
            Err(Refusal(format!("agents may not read {slug:?}'s log lines: a person lets them, under \"agents may read its logs\" in its deploy settings")))
        }
        _ => Ok(()),
    }
}

/// The roles of `held` bound where `on` is.
fn covering<'a>(held: &'a [Held], on: &'a Resource) -> impl Iterator<Item = &'a Arc<RoleDef>> + 'a {
    held.iter().filter(|h| h.covers(on)).map(|h| &h.role)
}

/// What a refusal by the roles says: the flow isn't the agent's own; the tokens aren't the
/// person's; that no role the caller holds there holds it (also when one of them is an admin's
/// own, the plan's D16); or what the action needs, "that needs an admin" when only the admin
/// role holds it.
fn refused(who: &Caller, action: Action, on: &Resource) -> Refusal {
    let name = who.name.as_deref().unwrap_or_default();
    if action == RunSteps {
        return Refusal(steps_refused(who, on));
    }
    let roles: Vec<&Arc<RoleDef>> = covering(who.holds.as_deref().unwrap_or_default(), on).collect();
    match on {
        Resource::Saved { name: flow, by } if roles.iter().any(|r| r.holds_own(action)) => {
            let whose = by.unwrap_or("someone");
            return Refusal(format!("a flow named \"{flow}\" saved by {whose} is already here, and an agent doesn't replace someone else's flow: save it under another name"));
        }
        Resource::Principal { .. } if who.kind == Some(Kind::Person) => {
            return Refusal(format!("{name} may not do that: an agent's or a service's tokens are its owner's or an admin's to make"));
        }
        _ => {}
    }
    // Deciding takes a role bound where the approval is, whatever else the caller holds there.
    if roles.is_empty() || roles.iter().any(|r| r.builtin.is_none()) || action == RunDecide {
        let place = match on {
            Resource::Host | Resource::Principal { .. } => "on this host".to_string(),
            Resource::Flows => "on any flow".to_string(),
            Resource::Tag(tag) => format!("on the flows tagged {tag:?}"),
            Resource::Version { slug, tags: [] } => format!("on {slug:?} without tags"),
            Resource::Version { slug, tags } => format!("on {slug:?} tagged {}", tags.join(", ")),
            Resource::Approval { slug, name: Some(point) } => format!("for {slug:?}'s approval {point:?}"),
            Resource::Flow(slug) | Resource::Saved { name: slug, .. } | Resource::Action { slug, .. } | Resource::Logs { slug, .. } | Resource::Approval { slug, .. } => format!("on {slug:?}"),
        };
        return Refusal(format!("{name} may not do that: no role {name} holds {place} holds {}", action.name()));
    }
    let only_admins = BUILTIN.iter().filter(|d| d.builtin != Some(Role::Admin)).all(|d| !d.holds_any(action));
    let what = if only_admins {
        ADMINS
    } else if roles.iter().any(|r| r.builtin == Some(Role::Agent)) {
        AGENTS
    } else {
        VIEWERS
    };
    Refusal(format!("{name} may not do that: {what}"))
}

/// What a refusal of a run's steps and values says (R55): that no role the caller holds there holds
/// run.steps, and what a person runs to give it them there, binding the built-in role that holds it
/// for their kind (the table's: an admin defines one when none does). The agent loop reads it at
/// `explain` and goes on from it.
fn steps_refused(who: &Caller, on: &Resource) -> String {
    let name = who.name.as_deref().unwrap_or_default();
    let (whose, scope) = match on {
        Resource::Flow(slug) => (format!("{slug:?}'s"), format!("--flow {slug}")),
        Resource::Tag(tag) => (format!("the flows tagged {tag:?}"), format!("--tag {tag}")),
        Resource::Flows => ("any flow's".to_string(), "--flow SLUG".to_string()),
        _ => ("these".to_string(), "--flow SLUG".to_string()),
    };
    let role = who.kind.and_then(|kind| BUILTIN.iter().filter(|d| d.builtin != Some(Role::Admin)).find(|d| d.holds_for(RunSteps, kind)));
    let give = match (role, who.kind) {
        (Some(role), _) => format!("a person gives it with `holon bind {} --to {name} {scope} --why …`, or in the editor: People, then {name}, then bind a role", role.name),
        (None, kind) => format!("no built-in role gives it to {}: an admin defines one that holds run.steps, and a person binds it to {name} there", kind.map_or("this caller", |k| k.said())),
    };
    format!("{name} may not read {whose} runs' steps and values (explain, a run's detail, export, replay): no role {name} holds there holds run.steps; {give}")
}

/// What a route asks of its caller.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Rule {
    /// Nothing: anyone, signed in or not (signing in, the health check, a CORS preflight).
    Open,
    /// That they are signed in: their own account, and signing out.
    SignedIn,
    /// That `can` lets them do the action to what the path names.
    Can(Action, On),
    /// That `can` lets them do either action to what the path names: the handler answers each
    /// what it may see (the waiting runs, to whoever may decide one of their approvals).
    Either(Action, On, Action, On),
    /// That `can` lets them do both actions to what the path names, the first asked first: a
    /// run's evidence is its steps (R55).
    Both(Action, Action, On),
}

/// What a route's path names: the resource its rule asks `can` about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum On {
    Host,
    Flows,
    /// The flow of the path's `{slug}`.
    Slug,
    /// Some approval point of the path's `{slug}`: a binding at one lets its holder reach the
    /// handler, which asks about the point the body names (durable.rs).
    Approvals,
    /// The principal of the path's `{name}`, with its owner from the accounts.
    Principal,
}

/// A route the control plane answers.
#[derive(Debug)]
pub(crate) struct Route {
    pub(crate) method: &'static str,
    /// `/deployments/{slug}/runs`: each `{…}` is one segment, never empty; `*` is any path.
    pub(crate) path: &'static str,
    pub(crate) rule: Rule,
}

const fn open(method: &'static str, path: &'static str) -> Route {
    Route { method, path, rule: Rule::Open }
}

const fn signed_in(method: &'static str, path: &'static str) -> Route {
    Route { method, path, rule: Rule::SignedIn }
}

const fn asks(method: &'static str, path: &'static str, action: Action, on: On) -> Route {
    Route { method, path, rule: Rule::Can(action, on) }
}

const fn asks_either(method: &'static str, path: &'static str, first: (Action, On), second: (Action, On)) -> Route {
    Route { method, path, rule: Rule::Either(first.0, first.1, second.0, second.1) }
}

const fn both(method: &'static str, path: &'static str, first: Action, second: Action, on: On) -> Route {
    Route { method, path, rule: Rule::Both(first, second, on) }
}

/// Every route, in the order the routers read them (auth.rs's, then http.rs's): the first that
/// matches is the route. A route a router answers without a row here is never reached.
pub(crate) static ROUTES: &[Route] = &[
    open("OPTIONS", "*"),
    // Signing in, and one's own account (auth.rs).
    open("GET", "/auth/state"),
    open("GET", "/auth/config"),
    open("POST", "/auth/setup"),
    open("POST", "/auth/sign-in"),
    open("GET", "/auth/invite"),
    open("POST", "/auth/invite"),
    signed_in("GET", "/auth/me"),
    // What its caller may do, action by action (bindings.rs).
    signed_in("GET", "/auth/me/can"),
    signed_in("GET", "/auth/owned"),
    signed_in("POST", "/auth/password"),
    signed_in("GET", "/auth/prefs"),
    signed_in("POST", "/auth/prefs"),
    signed_in("POST", "/auth/sign-out"),
    // The host's resources and pages (folders.rs, pick.rs, storage.rs, sweep.rs, databases.rs,
    // mails.rs, feeds.rs, queues.rs, grants.rs, deploy.rs's server secrets).
    asks("GET", "/admin/folders", ResourceView, On::Host),
    asks("POST", "/admin/folders", ResourceBind, On::Host),
    asks("POST", "/admin/folders/sweep", HostOperate, On::Host),
    asks("GET", "/admin/folders/browse", ResourceBind, On::Host),
    asks("POST", "/admin/folders/pick", ResourceBind, On::Host),
    asks("DELETE", "/admin/folders/{name}", ResourceUnbind, On::Host),
    asks("GET", "/admin/host", HostView, On::Host),
    asks("GET", "/admin/warnings", HostView, On::Host),
    asks("POST", "/admin/storage", HostOperate, On::Host),
    asks("POST", "/admin/store/sweep", HostOperate, On::Host),
    asks("GET", "/admin/keys", HostView, On::Host),
    asks("POST", "/admin/keys/rotate", HostOperate, On::Host),
    asks("POST", "/admin/keys/forget", HostOperate, On::Host),
    asks("GET", "/admin/databases", ResourceView, On::Host),
    asks("POST", "/admin/databases", ResourceBind, On::Host),
    asks("GET", "/admin/databases/{name}/tables", ResourceView, On::Host),
    asks("DELETE", "/admin/databases/{name}", ResourceUnbind, On::Host),
    asks("GET", "/admin/mails", ResourceView, On::Host),
    asks("POST", "/admin/mails", ResourceBind, On::Host),
    asks("DELETE", "/admin/mails/{name}", ResourceUnbind, On::Host),
    asks("GET", "/admin/feeds", ResourceView, On::Host),
    asks("POST", "/admin/feeds", ResourceBind, On::Host),
    asks("DELETE", "/admin/feeds/{name}", ResourceUnbind, On::Host),
    asks("GET", "/admin/queues", ResourceView, On::Host),
    asks("POST", "/admin/queues", ResourceBind, On::Host),
    asks("DELETE", "/admin/queues/{name}", ResourceUnbind, On::Host),
    asks("GET", "/admin/grants", HostView, On::Host),
    asks("GET", "/admin/secrets/env", ResourceView, On::Host),
    asks("GET", "/admin/secrets", ResourceView, On::Host),
    asks("GET", "/admin/secrets/{name}", ResourceView, On::Host),
    asks("POST", "/admin/secrets/{name}", ResourceBind, On::Host),
    asks("POST", "/admin/secrets/{name}/rollback", ResourceBind, On::Host),
    asks("POST", "/admin/secrets/{name}/cap", ResourceBind, On::Host),
    asks("DELETE", "/admin/secrets/{name}", ResourceUnbind, On::Host),
    // People (auth.rs).
    asks("GET", "/people", PeopleView, On::Host),
    asks("POST", "/people", PeopleManage, On::Host),
    asks("GET", "/admin/oidc", PeopleView, On::Host),
    asks("POST", "/admin/oidc", PeopleManage, On::Host),
    asks("GET", "/people/{name}/tokens", PeopleManage, On::Principal),
    asks("POST", "/people/{name}/tokens", PeopleManage, On::Principal),
    asks("DELETE", "/people/{name}/tokens/{id}", PeopleManage, On::Principal),
    asks("POST", "/people/{name}/reset", PeopleManage, On::Host),
    asks("DELETE", "/people/{name}", PeopleManage, On::Host),
    asks("PATCH", "/people/{name}", PeopleManage, On::Host),
    // What is bound to whom, and its versions (bindings.rs).
    asks("GET", "/bindings", PeopleView, On::Host),
    asks("POST", "/bindings", PeopleAssign, On::Host),
    asks("DELETE", "/bindings/{id}", PeopleAssign, On::Host),
    asks("GET", "/access/versions", PeopleView, On::Host),
    asks("GET", "/access", PeopleView, On::Host),
    asks("PUT", "/access/approvals", PeopleAssign, On::Host),
    // Access changes waiting for people (access_proposals.rs): approved and declined by a person
    // holding what each gives, as its handler asks.
    asks("GET", "/access/proposals", PeopleView, On::Host),
    asks("POST", "/access/proposals/{id}/approve", PeopleAssign, On::Host),
    asks("POST", "/access/proposals/{id}/decline", PeopleAssign, On::Host),
    asks("GET", "/access/local", PeopleView, On::Host),
    asks("PUT", "/access/local", PeopleAssign, On::Host),
    asks("GET", "/roles", RoleView, On::Host),
    asks("PUT", "/roles/{name}", RoleDefine, On::Host),
    asks("DELETE", "/roles/{name}", RoleDefine, On::Host),
    asks("GET", "/groups", PeopleView, On::Host),
    asks("PUT", "/groups/{name}", PeopleAssign, On::Host),
    asks("DELETE", "/groups/{name}", PeopleAssign, On::Host),
    // The rest (http.rs).
    open("GET", "/health"),
    signed_in("GET", "/version"),
    asks("GET", "/unit/{hash}", FlowView, On::Flows),
    asks("POST", "/unit", FlowPropose, On::Flows),
    asks("POST", "/deploy", FlowPropose, On::Flows),
    asks("POST", "/flows", FlowEdit, On::Flows),
    asks("GET", "/flows", FlowView, On::Flows),
    // A flow its caller saved and no one deployed is theirs wherever they hold the action, so its
    // handler asks about the flow once it knows whose it is (flows.rs).
    asks("GET", "/flows/{slug}", FlowView, On::Flows),
    asks("DELETE", "/flows/{slug}", FlowDelete, On::Flows),
    asks("GET", "/proposals", FlowView, On::Flows),
    asks("POST", "/proposals/{id}/decline", FlowApproveDeploy, On::Flows),
    asks("GET", "/proposals/{id}", FlowView, On::Flows),
    asks("GET", "/summary", RunView, On::Flows),
    asks("POST", "/summary/check", HostCheck, On::Host),
    asks("GET", "/activity", FlowView, On::Flows),
    asks("POST", "/deploy/preview", FlowPropose, On::Flows),
    asks("GET", "/permissions", PolicyView, On::Host),
    asks("POST", "/permissions/reload", PolicyEdit, On::Host),
    asks("GET", "/deployments", FlowView, On::Flows),
    // Every flow's waiting runs, each as its flow's runs list them to the caller (durable.rs).
    signed_in("GET", "/runs/waiting"),
    asks("GET", "/deployments/{slug}/secrets", SecretList, On::Slug),
    asks("GET", "/deployments/{slug}/secrets/{name}", SecretList, On::Slug),
    asks("POST", "/deployments/{slug}/secrets/{name}", SecretSet, On::Slug),
    asks("DELETE", "/deployments/{slug}/secrets/{name}", SecretSet, On::Slug),
    asks("POST", "/deployments/{slug}/secrets/{name}/rollback", SecretRollBack, On::Slug),
    asks("GET", "/deployments/{slug}/collections/{name}", StateView, On::Slug),
    // Its waiting runs: all of them to whoever views its runs, those waiting at an approval to
    // whoever may decide it (durable.rs).
    asks_either("GET", "/deployments/{slug}/runs", (RunView, On::Slug), (RunDecide, On::Approvals)),
    asks("GET", "/deployments/{slug}/logs", RunView, On::Slug),
    asks("GET", "/deployments/{slug}/feeds", StateView, On::Slug),
    asks("GET", "/deployments/{slug}/queues", StateView, On::Slug),
    asks("GET", "/deployments/{slug}/channels", StateView, On::Slug),
    asks("POST", "/deployments/{slug}/runs/{id}/decide", RunDecide, On::Approvals),
    // A cell's values, before and after each run that changed it: the state's (R56).
    asks("GET", "/deployments/{slug}/changes", StateView, On::Slug),
    // The last clock ticks, each with its outputs and effects: runs' values (R56, as R55's log).
    asks("GET", "/deployments/{slug}/history", RunSteps, On::Slug),
    asks("GET", "/deployments/{slug}/state", StateView, On::Slug),
    // A run's steps and values are run.steps' (R55): the whole log, and the evidence made of it.
    // The list of runs, each without its values, is run.view's.
    asks("GET", "/deployments/{slug}/log", RunSteps, On::Slug),
    asks("GET", "/deployments/{slug}/log/heads", RunView, On::Slug),
    both("GET", "/deployments/{slug}/export", RunSteps, RunExport, On::Slug),
    both("GET", "/deployments/{slug}/replay", RunSteps, RunExport, On::Slug),
    asks("GET", "/deployments/{slug}/text", FlowView, On::Slug),
    asks("GET", "/deployments/{slug}/events", FlowView, On::Slug),
    asks("GET", "/deployments/{slug}/versions", FlowView, On::Slug),
    asks("GET", "/deployments/{slug}/holds", StateView, On::Slug),
    // With "state": true, its handler asks state.view as well: a state's values (R57).
    asks("POST", "/deployments/{slug}/restore", FlowRestoreDryRun, On::Slug),
    asks("GET", "/deployments/{slug}/manifest", FlowView, On::Slug),
    asks("GET", "/deployments/{slug}/grant", FlowView, On::Slug),
    asks("POST", "/deployments/{slug}/grant", FlowApproveDeploy, On::Slug),
    // Each session with the state it ended with (R56).
    asks("GET", "/deployments/{slug}/sessions", StateView, On::Slug),
    asks("POST", "/deployments/{slug}/run", FlowLifecycle, On::Slug),
    asks("POST", "/deployments/{slug}/pause", FlowLifecycle, On::Slug),
    asks("POST", "/deployments/{slug}/stop", FlowLifecycle, On::Slug),
    asks("POST", "/deployments/{slug}/request", RunCall, On::Slug),
    asks("POST", "/deployments/{slug}/settings", FlowDeploy, On::Slug),
    // Its outputs, a run's values, only to whoever holds run.steps: to anyone else, left out (R57).
    asks("POST", "/deployments/{slug}/tick", FlowLifecycle, On::Slug),
    asks("DELETE", "/deployments/{slug}", FlowDelete, On::Slug),
    // It answers how its actions are run: a deployer's question (the plan's D4).
    asks("POST", "/trigger/{slug}", FlowDeploy, On::Slug),
    asks("POST", "/trigger/{slug}/{name}", RunCall, On::Slug),
];

/// The parts of a path its route names.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Parts<'a> {
    pub(crate) slug: Option<&'a str>,
    pub(crate) name: Option<&'a str>,
}

/// The route `method` and `url` ask for, and the parts of the path it names; none for a path no
/// route has.
pub(crate) fn find<'a>(method: &str, url: &'a str) -> Option<(&'static Route, Parts<'a>)> {
    let url = url.split('?').next().unwrap_or_default();
    ROUTES.iter().filter(|r| r.method == method).find_map(|r| if r.path == "*" { Some((r, Parts::default())) } else { matched(r.path, url).map(|parts| (r, parts)) })
}

/// The parts of `url` that `pattern` names, if it matches.
fn matched<'a>(pattern: &str, url: &'a str) -> Option<Parts<'a>> {
    let (mut want, mut got) = (pattern.split('/'), url.split('/'));
    let mut parts = Parts::default();
    loop {
        match (want.next(), got.next()) {
            (None, None) => return Some(parts),
            (Some(w), Some(g)) if w.starts_with('{') && !g.is_empty() => match w {
                "{slug}" => parts.slug = Some(g),
                "{name}" => parts.name = Some(g),
                _ => {}
            },
            (Some(w), Some(g)) if w == g => {}
            _ => return None,
        }
    }
}

/// Whether `who` may take `route`, as its rule says, on what `parts` name. Someone not signed in
/// was refused before (access.rs), so a route open to anyone signed in has nothing to ask.
pub(crate) fn may(who: &Caller, route: &Route, parts: &Parts, accounts: Option<&Accounts>) -> Result<(), Refusal> {
    match route.rule {
        Rule::Can(action, on) => may_on(who, action, on, parts, accounts),
        Rule::Either(first, on, second, then) => may_on(who, first, on, parts, accounts).or_else(|refused| may_on(who, second, then, parts, accounts).map_err(|_| refused)),
        Rule::Both(first, second, on) => may_on(who, first, on, parts, accounts).and_then(|()| may_on(who, second, on, parts, accounts)),
        Rule::Open | Rule::SignedIn => Ok(()),
    }
}

/// Whether `who` may do `action` to what `on` names of `parts`.
fn may_on(who: &Caller, action: Action, on: On, parts: &Parts, accounts: Option<&Accounts>) -> Result<(), Refusal> {
    let owner;
    let resource = match on {
        On::Host => Resource::Host,
        On::Flows => Resource::Flows,
        On::Slug => Resource::Flow(parts.slug.unwrap_or_default()),
        On::Approvals => Resource::Approval { slug: parts.slug.unwrap_or_default(), name: None },
        On::Principal => {
            let name = parts.name.unwrap_or_default();
            owner = accounts.and_then(|a| a.principal(name)).and_then(|p| p.owner);
            Resource::Principal { name, owner: owner.as_deref() }
        }
    };
    can(who, action, &resource).map_err(|Refusal(said)| {
        // Giving a run's steps is an access change: say so where one waits for people (R55).
        let waits = accounts.and_then(|a| a.access_document().approvals.changes).filter(|_| action == RunSteps);
        Refusal(match waits {
            Some(n) => format!("{said}. Here an access change that gives more waits for {n} person(s) besides its author, in Approvals, Access changes"),
            None => said,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// The design, which names the catalogue (§4) and the built-in roles (§5.1).
    const DESIGN: &str = include_str!("../../../docs/L4s/access-and-approvals-L4.md");

    /// The rows of the first table in the design's section that starts with `heading`: each one's
    /// cells.
    fn table(heading: &str) -> Vec<Vec<&'static str>> {
        let section = DESIGN.split(heading).nth(1).unwrap_or_else(|| panic!("the design has no {heading:?}"));
        section
            .lines()
            .skip_while(|l| !l.starts_with('|'))
            .take_while(|l| l.starts_with('|'))
            .skip(2)
            .map(|l| l.trim().trim_matches('|').split('|').map(str::trim).collect())
            .collect()
    }

    /// Each `quoted` word in `text`, and what follows it up to the next.
    fn quoted(text: &str) -> Vec<(&str, &str)> {
        text.split('`').skip(1).collect::<Vec<_>>().chunks(2).map(|c| (c[0], c.get(1).copied().unwrap_or_default())).collect()
    }

    fn caller(name: &str, kind: Kind, role: Role) -> Caller {
        Caller::holding(name, kind, role)
    }

    /// Every resource of each kind, for an action to be asked of.
    fn resources() -> Vec<Resource<'static>> {
        vec![
            Resource::Host,
            Resource::Flows,
            Resource::Flow("d"),
            Resource::Saved { name: "f", by: Some("ada") },
            Resource::Action { slug: "d", name: "echo", agents_may: true },
            Resource::Logs { slug: "d", agents_may: true },
            Resource::Approval { slug: "d", name: Some("ok") },
            Resource::Principal { name: "ci", owner: Some("bob") },
        ]
    }

    /// The catalogue is the design's §4, name for name.
    #[test]
    fn the_catalogue_is_the_designs() {
        let designed: BTreeSet<String> = table("## 4. Permissions")
            .iter()
            .flat_map(|row| {
                let resource = quoted(row[0])[0].0;
                quoted(row[1]).into_iter().map(move |(action, _)| format!("{resource}.{action}"))
            })
            .collect();
        let here: BTreeSet<String> = Action::ALL.iter().map(|a| a.name().to_string()).collect();
        assert_eq!(here, designed);
        assert_eq!(here.len(), Action::ALL.len(), "each action named once");
    }

    /// The built-in roles are the design's §5.1, permission for permission and kind for kind.
    #[test]
    fn the_built_in_roles_are_the_designs() {
        let mut designed: BTreeMap<&str, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
        for row in table("### 5.1 Roles") {
            let name = quoted(row[0])[0].0;
            let mut holds = BTreeSet::new();
            if row[1].starts_with("viewer's") {
                holds.extend(designed["viewer"].0.iter().cloned());
            }
            if row[1] == "everything" {
                holds.extend(Action::ALL.iter().map(|a| a.name().to_string()));
            }
            for (permission, after) in quoted(row[1]) {
                holds.insert(if after.trim_start().starts_with("(its own)") { format!("{permission} (its own)") } else { permission.to_string() });
            }
            let kinds = if row[2] == "any" { "person, agent, service" } else { row[2] };
            designed.insert(name, (holds, kinds.split(", ").map(String::from).collect()));
        }
        assert_eq!(designed.len(), BUILTIN.len());
        for def in BUILTIN.iter() {
            let name = serde_json::to_value(def.builtin.unwrap()).unwrap().as_str().unwrap().to_string();
            let holds: BTreeSet<String> = def
                .holds
                .iter()
                .flat_map(|h| match *h {
                    Holds::All => Action::ALL.iter().map(|a| a.name().to_string()).collect(),
                    Do(a) => vec![a.name().to_string()],
                    Own(a) => vec![format!("{} (its own)", a.name())],
                    // To the role's every kind, as the design writes it; to fewer, it names them.
                    For(a, kinds) if kinds == ANYONE => vec![a.name().to_string()],
                    For(a, kinds) => vec![format!("{} ({})", a.name(), kinds.iter().map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string()).collect::<Vec<_>>().join(", "))],
                })
                .collect();
            let kinds: BTreeSet<String> = def.kinds.iter().map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string()).collect();
            assert_eq!(Some(&(holds, kinds)), designed.get(name.as_str()), "{name}");
            assert_eq!(role(def.builtin.unwrap()).name, name);
        }
    }

    /// `can` is the union of the bindings that cover what is asked (the design's §5.3): for every
    /// set of at most two bindings, of the built-in roles and two of an admin's at each kind of
    /// scope, bound to the caller or to a group of theirs, held by each kind of principal, and every
    /// action on every resource it is done to, it lets through exactly when a binding whose scope
    /// covers the resource, as the table here says, is of a role that holds the action and that the
    /// caller's kind may hold, or the caller owns it; and the fixed rules let it. A binding that
    /// has expired holds nothing. Refused with no binding covering it, the caller is told so, in
    /// words naming where; and it may do an action everywhere only by a binding on the host.
    #[test]
    fn can_is_the_union_of_the_bindings_that_cover_it() {
        use crate::bindings::{Bindings, Document, Group, RoleText, Scope, Selector, Subject};
        let slug = |s: &str| Selector::Slug(s.into());
        let scopes = [
            ('H', Scope::Host),
            ('D', Scope::Flows(slug("d"))),
            ('E', Scope::Flows(slug("e"))),
            ('A', Scope::Approval { flow: slug("d"), name: "ok".into() }),
        ];
        // Each resource, the scopes that cover it (by their letters above), and where a refusal
        // says it is.
        let covered = [
            (Resource::Host, "H", "on this host"),
            (Resource::Flows, "HDE", "on any flow"),
            (Resource::Flow("d"), "HD", "on \"d\""),
            (Resource::Flow("e"), "HE", "on \"e\""),
            (Resource::Saved { name: "d", by: Some("ada") }, "HD", "on \"d\""),
            (Resource::Saved { name: "e", by: Some("eve") }, "HE", "on \"e\""),
            (Resource::Action { slug: "d", name: "echo", agents_may: false }, "HD", "on \"d\""),
            (Resource::Action { slug: "e", name: "echo", agents_may: true }, "HE", "on \"e\""),
            (Resource::Logs { slug: "d", agents_may: false }, "HD", "on \"d\""),
            (Resource::Logs { slug: "e", agents_may: true }, "HE", "on \"e\""),
            (Resource::Approval { slug: "d", name: Some("ok") }, "HDA", "for \"d\"'s approval \"ok\""),
            (Resource::Approval { slug: "d", name: Some("no") }, "HD", "for \"d\"'s approval \"no\""),
            (Resource::Approval { slug: "d", name: None }, "HDA", "on \"d\""),
            (Resource::Approval { slug: "e", name: None }, "HE", "on \"e\""),
            (Resource::Principal { name: "ci", owner: Some("eve") }, "H", "on this host"),
            (Resource::Principal { name: "scout", owner: Some("cy") }, "H", "on this host"),
        ];
        // An admin's roles: a person's, and one any kind may hold that allows a person's action
        // and one of the host's.
        let custom = [
            RoleText { name: "approver".into(), about: None, allows: vec!["run.decide".into(), "run.view".into()], kinds: vec![Kind::Person] },
            RoleText { name: "operator".into(), about: None, allows: vec!["flow.lifecycle".into(), "flow.view".into(), "people.view".into()], kinds: ANYONE.to_vec() },
        ];
        let roles: Vec<Arc<RoleDef>> = BUILTIN.iter().cloned().chain(custom.iter().map(|r| Arc::new(r.def().unwrap()))).collect();
        let bindings: Vec<(char, Held)> = scopes.iter().flat_map(|(at, scope)| roles.iter().map(|d| (*at, Held::new(Arc::clone(d), scope.clone())))).collect();
        let mut sets: Vec<Vec<(char, Held)>> = vec![vec![]];
        for (i, one) in bindings.iter().enumerate() {
            sets.push(vec![one.clone()]);
            sets.extend(bindings[i + 1..].iter().map(|two| vec![one.clone(), two.clone()]));
        }
        assert_eq!(sets.len(), 1 + 24 + 24 * 23 / 2);
        let now = 1_800_000_000;
        for &kind in ANYONE {
            for set in &sets {
                for group in [None, Some("team")] {
                    // What eve holds, as the document says it: the set, bound to eve or to eve's
                    // group, and admin on the host, expired now.
                    let mut d = Document::default();
                    d.roles = custom.to_vec();
                    d.groups.push(Group { name: "team".into(), about: None, members: vec!["cy".into(), "eve".into()] });
                    d.bind(Subject::Principal("eve".into()), "admin", Scope::Host, Some(now), "ada", 1);
                    for (_, h) in set {
                        let to = group.map_or(Subject::Principal("eve".into()), |g| Subject::Group(g.into()));
                        d.bind(to, &h.role.name, h.scope.clone(), None, "ada", 1);
                    }
                    let access = Bindings::memory();
                    access.keep(d).unwrap();
                    let who = Caller { name: Some("eve".into()), kind: Some(kind), holds: Some(access.held("eve", now)), local: None };
                    let case = |what: &str| {
                        let held: Vec<String> = set.iter().map(|(at, h)| format!("{} at {at}", h.role.name)).collect();
                        format!("{what}, {kind:?} holding {held:?}{}", group.map(|g| format!(" through {g}")).unwrap_or_default())
                    };
                    for (on, letters, place) in &covered {
                        for action in Action::ALL.into_iter().filter(|a| on.takes(*a)) {
                            let covering: Vec<&Held> = set.iter().filter(|(at, _)| letters.contains(*at)).map(|(_, h)| h).collect();
                            let by_role = covering.iter().any(|h| h.role.kinds.contains(&kind) && h.role.holds(action, on, &who));
                            let expected = (by_role || owns(&who, action, on)) && fixed(&who, action, on).is_ok();
                            let got = can(&who, action, on);
                            assert_eq!(got.is_ok(), expected, "{}", case(&format!("{} on {on:?}", action.name())));
                            let told_whose = matches!(on, Resource::Principal { .. }) && kind == Kind::Person;
                            if covering.is_empty() && !expected && !owns(&who, action, on) && !told_whose {
                                let said = if action == RunSteps { steps_refused(&who, on) } else { format!("eve may not do that: no role eve holds {place} holds {}", action.name()) };
                                assert_eq!(got.unwrap_err().0, said, "{}", case(""));
                                // A person is told the command that gives them run.steps: viewer.
                                if action == RunSteps && kind == Kind::Person {
                                    assert!(said.starts_with("eve may not read ") && said.contains("holon bind viewer --to eve"), "{said}");
                                }
                            }
                        }
                        // An approval's step with a role takes someone bound to it by name where the
                        // approval is: holding everything (admin) isn't holding approver.
                        if matches!(on, Resource::Approval { .. }) {
                            for role in &roles {
                                let bound = set.iter().any(|(at, h)| letters.contains(*at) && h.role.name == role.name && h.role.kinds.contains(&kind));
                                assert_eq!(holding(&who, Some(&role.name), on).is_some(), bound, "{}", case(&format!("bound to {} on {on:?}", role.name)));
                            }
                        }
                    }
                    let at_host = set.iter().any(|(at, h)| *at == 'H' && h.role.kinds.contains(&kind) && h.role.holds_any(FlowView));
                    assert_eq!(everywhere(&who, FlowView), at_host, "{}", case("everywhere"));
                    // The expired binding held until it expired, saying when; it stays in the document.
                    if set.is_empty() && group.is_none() && kind == Kind::Person {
                        assert!(access.held("eve", now - 1).iter().any(|h| h.role.name == "admin" && h.until == Some(now)));
                        assert!(!access.held("eve", now).iter().any(|h| h.role.name == "admin"));
                        assert_eq!(access.document().bindings.len(), 1, "kept until unbound");
                    }
                }
            }
        }
        // Every action is done to something the table holds, and the admin may do it there.
        let ada = caller("ada", Kind::Person, Role::Admin);
        for action in Action::ALL {
            assert!(covered.iter().any(|(on, _, _)| on.takes(action)), "{} is done to something", action.name());
            for r in resources().iter().filter(|r| r.takes(action)) {
                assert_eq!(can(&ada, action, r), Ok(()), "the admin: {} on {r:?}", action.name());
            }
        }
        // An agent's own role edits only what it saved: not another's, nor what no one saved.
        let claude = caller("claude", Kind::Agent, Role::Agent);
        for by in [Some("ada"), None] {
            let said = can(&claude, FlowEdit, &Resource::Saved { name: "f", by }).unwrap_err().0;
            assert!(said.contains("an agent doesn't replace someone else's flow"), "{said}");
        }
        // A group's binding: reaching names the group; a member forgotten holds nothing by it, and
        // the other member still does.
        let mut d = Document::default();
        d.groups.push(Group { name: "team".into(), about: None, members: vec!["cy".into(), "eve".into()] });
        d.bind(Subject::Group("team".into()), "viewer", Scope::Host, None, "ada", 1);
        let access = Bindings::memory();
        access.keep(d.clone().after(&Document::default(), "ada", "bound", None, 1)).unwrap();
        assert_eq!(access.document().reaching("cy").map(|(b, g)| (b.id, g)).collect::<Vec<_>>(), [(0, Some("team"))]);
        d.forget("cy");
        access.keep(d.after(&access.document(), "ada", "removed cy", None, 2)).unwrap();
        assert!(access.held("cy", now).is_empty());
        assert_eq!(access.held("eve", now).len(), 1);
    }

    /// No role gives an agent what is a person's (the plan's D20): whatever it holds, an admin's
    /// role allowing everything on the host included, it never approves a deploy, starts, stops,
    /// deletes or restores a flow, writes a secret, or does the host's people, roles, resources or
    /// operations; and every other action such a role allows it, it may.
    #[test]
    fn no_role_gives_an_agent_what_is_a_persons() {
        let all = Arc::new(RoleDef::custom("everything", &Action::ALL, ANYONE));
        let held = |kind: Kind| Caller { name: Some("scout".into()), kind: Some(kind), holds: Some(Arc::from([Held::new(Arc::clone(&all), crate::bindings::Scope::Host)])), local: None };
        let (agent, service) = (held(Kind::Agent), held(Kind::Service));
        let persons = ["flow.approve-deploy", "flow.lifecycle", "flow.delete", "flow.restore", "secret.set", "secret.roll-back", "host.operate"];
        for action in Action::ALL {
            let of = action.name().split('.').next().unwrap();
            let a_persons = persons.contains(&action.name()) || ["people", "role", "resource"].contains(&of);
            for on in resources().iter().filter(|r| r.takes(action) && !matches!(r, Resource::Action { agents_may: false, .. } | Resource::Logs { agents_may: false, .. })) {
                let said = can(&agent, action, on).err().map(|r| r.0);
                if action == RunDecide {
                    assert_eq!(said, None, "an agent decides the steps written for an agent (durable.rs): {on:?}");
                } else if a_persons {
                    assert_eq!(said, Some(format!("scout may not do that: {AGENTS}")), "{} on {on:?}", action.name());
                } else {
                    assert_eq!(said, None, "{} on {on:?}", action.name());
                }
                if action != RunDecide {
                    assert_eq!(can(&service, action, on), Ok(()), "a service holding it may: {} on {on:?}", action.name());
                }
            }
        }
    }

    /// Every route can be reached: no row before it matches its path, with each part named; and
    /// what its rule asks is an action done to what its path names.
    #[test]
    fn every_route_is_reached_and_asks_of_what_it_names() {
        for route in ROUTES {
            let path = route.path.split('/').map(|s| if s.starts_with('{') { "x" } else { s }).collect::<Vec<_>>().join("/");
            let (found, parts) = find(route.method, &path).unwrap_or_else(|| panic!("{} {}: no route", route.method, route.path));
            assert!(std::ptr::eq(found, route), "{} {} is taken by {} {}", route.method, route.path, found.method, found.path);
            let asked = match route.rule {
                Rule::Can(action, on) => vec![(action, on)],
                Rule::Either(first, on, second, then) => vec![(first, on), (second, then)],
                Rule::Both(first, second, on) => vec![(first, on), (second, on)],
                Rule::Open | Rule::SignedIn => vec![],
            };
            for (action, on) in asked {
                let resource = match on {
                    On::Host => Resource::Host,
                    On::Flows => Resource::Flows,
                    On::Slug => Resource::Flow("x"),
                    On::Approvals => Resource::Approval { slug: "x", name: None },
                    On::Principal => Resource::Principal { name: "x", owner: None },
                };
                assert!(resource.takes(action), "{} {}: {} isn't done to {resource:?}", route.method, route.path, action.name());
                let named = match on {
                    On::Slug | On::Approvals => parts.slug,
                    On::Principal => parts.name,
                    On::Host | On::Flows => Some("x"),
                };
                assert_eq!(named, Some("x"), "{} {} names what it asks of", route.method, route.path);
            }
        }
        assert_eq!(find("GET", "/deployments/d/runs?after=3").map(|(r, p)| (r.path, p.slug)), Some(("/deployments/{slug}/runs", Some("d"))), "the query isn't the path");
        for (method, path) in [("GET", "/deployments//runs"), ("GET", "/deployments/d/runs/"), ("GET", "/deployments/a/b/runs"), ("POST", "/flows/f"), ("GET", "/nope")] {
            assert!(find(method, path).is_none(), "{method} {path}");
        }
        assert_eq!(find("OPTIONS", "/anything/at/all").map(|(r, _)| r.path), Some("*"));
    }
}
