//! What each principal holds, and where (docs/L4s/access-and-approvals-L4.md §5 and §6): roles an
//! admin defines, groups of principals, and bindings of a role to a principal or a group at a
//! scope, kept in one document, `access.json` beside the accounts file (`.holon-access.json`
//! beside `.holon-accounts.json`). Every change makes a new version, with who made it, when, what
//! it changed and why, and every version is kept for good, one per line, in
//! `access.versions.jsonl` beside it.
//!
//! A scope is the host (everything), a flow by its slug (what is of that flow, and the lists,
//! which keep the flows their caller may see), or one approval point of a flow (deciding it, and
//! nothing else). A binding may expire: from then on it holds nothing, and stays in the document
//! until it is unbound. A binding to a group holds for each of its members whose kind its role
//! allows; a group's members are principals, never groups (§5.2).
//!
//! The accounts book (accounts.rs) keeps who the principals are, and owns this document: a change
//! to both, as inviting someone with a role, is made under the book's lock, checked whole
//! (`Document::check`), and written so that no binding on disk names a principal the book doesn't
//! have (accounts.rs `change_access`). The routes that read and change it are at the end of this
//! file. Scopes by tag come next (docs/L4s/roles-and-bindings-plan.md, step 3d).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rouille::{Request, Response};
use serde::{Deserialize, Serialize};

use crate::access::{Caller, Role};
use crate::accounts::Kind;
use crate::can::{can, Action, Resource, RoleDef};
use crate::http::{err, read_body};

/// One version of the access document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    /// 0 for a server that has none yet.
    pub(crate) version: u64,
    #[serde(default)]
    pub(crate) at: u64,
    /// Who made this version.
    #[serde(default)]
    pub(crate) by: String,
    /// What it changed, in words: "bound editor to bob on this host".
    #[serde(default)]
    pub(crate) what: String,
    /// Why, as its author said, if they did ("on call this week").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
    /// The roles an admin defined; the built-in ones are can.rs's.
    #[serde(default)]
    pub(crate) roles: Vec<RoleText>,
    #[serde(default)]
    pub(crate) groups: Vec<Group>,
    #[serde(default)]
    pub(crate) bindings: Vec<Binding>,
    /// The next binding's id: ids are never reused, so an old version's ids name what they named.
    #[serde(default)]
    next_id: u64,
}

/// A role an admin defines (§5.1), as kept: what it allows, by the catalogue's names, and which
/// kinds of principal may hold it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoleText {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) about: Option<String>,
    pub(crate) allows: Vec<String>,
    pub(crate) kinds: Vec<Kind>,
}

impl RoleText {
    /// It, as `can` reads it; or why it isn't a role: a built-in role's name, an action the
    /// catalogue doesn't name, none at all, or no kind to hold it.
    pub(crate) fn def(&self) -> Result<RoleDef, String> {
        let name = &self.name;
        if crate::can::builtin(name).is_some() {
            return Err(format!("{name} is a built-in role: name yours otherwise"));
        }
        crate::accounts::valid_name(name)?;
        let allows = self.allows.iter().map(|a| Action::named(a).ok_or(format!("{a:?} isn't an action: the catalogue names them as flow.deploy, run.decide…"))).collect::<Result<Vec<_>, _>>()?;
        if allows.is_empty() {
            return Err(format!("{name} allows nothing: give it an action"));
        }
        if self.kinds.is_empty() {
            return Err(format!("no kind may hold {name}: give it person, agent or service"));
        }
        Ok(RoleDef::custom(name, &allows, &self.kinds))
    }
}

/// A group of principals (§5.2): a binding to it holds for each member whose kind its role allows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Group {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) about: Option<String>,
    #[serde(default)]
    pub(crate) members: Vec<String>,
}

/// A role bound to a principal or a group at a scope, until it expires if it does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub(crate) id: u64,
    pub(crate) who: Subject,
    /// A built-in role's name, or one the document defines.
    pub(crate) role: String,
    pub(crate) scope: Scope,
    pub(crate) by: String,
    pub(crate) at: u64,
    /// When it stops holding (seconds), if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at: Option<u64>,
}

impl Binding {
    /// Whether it holds at `at` (seconds): until it expires.
    pub(crate) fn live(&self, at: u64) -> bool {
        self.expires_at.is_none_or(|until| at < until)
    }
}

/// Whom a binding is to. On disk: `{"principal": "bob"}`, `{"group": "payments"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Subject {
    Principal(String),
    Group(String),
}

impl Subject {
    /// As a version's words say it: `bob`, `the group payments`.
    pub(crate) fn said(&self) -> String {
        match self {
            Subject::Principal(name) => name.clone(),
            Subject::Group(name) => format!("the group {name}"),
        }
    }
}

/// Which flows a scope is of: one, by its slug.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Selector {
    Slug(String),
}

impl Selector {
    pub(crate) fn selects(&self, slug: &str) -> bool {
        match self {
            Selector::Slug(s) => s == slug,
        }
    }
}

/// Where a binding holds. On disk: `"host"`, `{"flows": {"slug": "payout"}}`,
/// `{"approval": {"flow": {"slug": "payout"}, "name": "ok"}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Scope {
    /// Everywhere: the host, every flow, every approval.
    Host,
    /// The flows it selects: what is of each (its runs, state, secrets and approvals), and the
    /// lists, which keep the flows their caller may see.
    Flows(Selector),
    /// One approval point of the flows it selects: deciding it, and nothing else (§5.3).
    Approval { flow: Selector, name: String },
}

impl Scope {
    /// As a version's words and a refusal say it: `on "payout"`.
    pub(crate) fn said(&self) -> String {
        match self {
            Scope::Host => "on this host".into(),
            Scope::Flows(Selector::Slug(slug)) => format!("on {slug:?}"),
            Scope::Approval { flow: Selector::Slug(slug), name } => format!("for {slug:?}'s approval {name:?}"),
        }
    }

    /// Why it isn't a scope here, if it isn't: a slug or an approval point's name as they are
    /// written.
    fn check(&self) -> Result<(), String> {
        let named = |what: &str, name: &str| if crate::deploy::is_valid_slug(name) { Ok(()) } else { Err(format!("{name:?} isn't {what}")) };
        match self {
            Scope::Host => Ok(()),
            Scope::Flows(Selector::Slug(slug)) => named("a flow's slug", slug),
            Scope::Approval { flow: Selector::Slug(slug), name } => named("a flow's slug", slug).and(named("an approval point's name", name)),
        }
    }
}

/// A binding as `can` reads it: the role, and where.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Held {
    pub(crate) role: Arc<RoleDef>,
    pub(crate) scope: Scope,
}

impl Held {
    /// What the OS user's run file holds (local.rs): admin, on the host, while the server runs.
    /// It is never in the document.
    pub(crate) fn admin() -> Arc<[Held]> {
        Arc::from([Held { role: Arc::clone(crate::can::role(Role::Admin)), scope: Scope::Host }])
    }

    /// Whether it holds on `on` (the plan's §2): the host covers everything; a flow's scope what
    /// is of that flow, and `Flows`, some flow, so its holder reaches the lists; an approval
    /// point deciding it, and the decide route's question, some approval of its flow.
    pub(crate) fn covers(&self, on: &Resource<'_>) -> bool {
        match &self.scope {
            Scope::Host => true,
            Scope::Flows(flows) => match on {
                Resource::Flows => true,
                Resource::Host | Resource::Principal { .. } => false,
                Resource::Flow(slug) | Resource::Saved { name: slug, .. } | Resource::Action { slug, .. } | Resource::Logs { slug, .. } | Resource::Approval { slug, .. } => flows.selects(slug),
            },
            Scope::Approval { flow, name } => matches!(on, Resource::Approval { slug, name: at } if flow.selects(slug) && at.is_none_or(|at| at == name)),
        }
    }
}

/// The widest built-in role of `held` on the host that `kind` may hold (admin, editor, agent, then
/// viewer): what the editor shows a principal as, until it asks what it may do
/// (`GET /auth/me/can`).
pub(crate) fn host_role(held: &[Held], kind: Kind) -> Option<Role> {
    let rank = |r: &Role| match r {
        Role::Admin => 3,
        Role::Editor => 2,
        Role::Agent => 1,
        Role::Viewer => 0,
    };
    held.iter().filter(|h| h.scope == Scope::Host && h.role.kinds.contains(&kind)).filter_map(|h| h.role.builtin).max_by_key(rank)
}

/// Why `kind` may not hold `role`, if it may not: a built-in role's words (accounts.rs), or the
/// kinds an admin's role names.
fn may_hold(kind: Kind, role: &RoleDef) -> Result<(), String> {
    match role.builtin {
        Some(builtin) => crate::accounts::held(kind, builtin),
        None if role.kinds.contains(&kind) => Ok(()),
        None => Err(format!("{} may not hold {}: it is held by {}", kind.said(), role.name, role.kinds.iter().map(|k| k.said()).collect::<Vec<_>>().join(" or "))),
    }
}

impl Document {
    /// `name`'s own bindings, not its groups' (the tests).
    #[cfg(test)]
    pub(crate) fn of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Binding> + 'a {
        self.bindings.iter().filter(move |b| matches!(&b.who, Subject::Principal(n) if n == name))
    }

    /// What holds for `name`: its own bindings, and its groups', with the group's name.
    pub(crate) fn reaching<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (&'a Binding, Option<&'a str>)> + 'a {
        self.bindings.iter().filter_map(move |b| match &b.who {
            Subject::Principal(n) => (n == name).then_some((b, None)),
            Subject::Group(g) => self.group(g).filter(|g| g.members.iter().any(|m| m == name)).map(|g| (b, Some(g.name.as_str()))),
        })
    }

    /// The role named `name`: a built-in one, or one this defines.
    pub(crate) fn role(&self, name: &str) -> Option<Arc<RoleDef>> {
        crate::can::builtin(name).cloned().or_else(|| self.roles.iter().find(|r| r.name == name).and_then(|r| r.def().ok()).map(Arc::new))
    }

    pub(crate) fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }

    /// How many bindings hold the role, or are to the group, `name`.
    pub(crate) fn bound(&self, role: Option<&str>, group: Option<&str>) -> usize {
        self.bindings.iter().filter(|b| role.is_some_and(|r| b.role == r) || group.is_some_and(|g| b.who == Subject::Group(g.into()))).count()
    }

    /// This, as the version after `before`: made by `by` at `at`, saying `what` and `why`.
    pub(crate) fn after(self, before: &Document, by: &str, what: &str, why: Option<&str>, at: u64) -> Document {
        Document { version: before.version + 1, at, by: by.into(), what: what.into(), why: why.map(String::from), ..self }
    }

    /// Bind `role` to `who` at `scope`, until `expires_at` if given: the new binding's id.
    pub(crate) fn bind(&mut self, who: Subject, role: &str, scope: Scope, expires_at: Option<u64>, by: &str, at: u64) -> u64 {
        let id = self.next_id;
        self.bindings.push(Binding { id, who, role: role.into(), scope, by: by.into(), at, expires_at });
        self.next_id += 1;
        id
    }

    /// Bind `role` to `name` at the host, in place of the roles bound to it there (none changes
    /// if that is the one bound there already).
    pub(crate) fn set_host_role(&mut self, name: &str, role: Role, by: &str, at: u64) {
        let host = |b: &Binding| matches!(&b.who, Subject::Principal(n) if n == name) && b.scope == Scope::Host;
        if self.bindings.iter().filter(|b| host(b)).map(|b| (b.role.as_str(), b.expires_at)).eq([(role.name(), None)]) {
            return;
        }
        self.bindings.retain(|b| !host(b));
        self.bind(Subject::Principal(name.into()), role.name(), Scope::Host, None, by, at);
    }

    /// Unbind the binding `id`: it, if it was there.
    pub(crate) fn unbind(&mut self, id: u64) -> Option<Binding> {
        let at = self.bindings.iter().position(|b| b.id == id)?;
        Some(self.bindings.remove(at))
    }

    /// Unbind everything bound to `name`, and take it out of its groups.
    pub(crate) fn forget(&mut self, name: &str) {
        self.bindings.retain(|b| !matches!(&b.who, Subject::Principal(n) if n == name));
        for g in &mut self.groups {
            g.members.retain(|m| m != name);
        }
    }

    /// Why this isn't a document the server keeps, if it isn't: each role defined once, and a role
    /// (`RoleText::def`); each group named once, as a name is written, its members principals
    /// here; each binding of a role here, to a group here or to a principal here whose kind may
    /// hold it. `kind_of` says which principals are here, and their kind.
    pub(crate) fn check(&self, kind_of: impl Fn(&str) -> Option<Kind>) -> Result<(), String> {
        let mut roles = HashSet::new();
        for r in &self.roles {
            r.def()?;
            if !roles.insert(r.name.as_str()) {
                return Err(format!("the role {} is defined twice", r.name));
            }
        }
        let mut groups = HashSet::new();
        for g in &self.groups {
            crate::accounts::valid_name(&g.name)?;
            if !groups.insert(g.name.as_str()) {
                return Err(format!("the group {} is made twice", g.name));
            }
            if let Some(m) = g.members.iter().find(|m| kind_of(m).is_none()) {
                return Err(format!("no principal {m:?}: a group's members are principals here"));
            }
        }
        for b in &self.bindings {
            let role = self.role(&b.role).ok_or(format!("no role {:?}", b.role))?;
            match &b.who {
                Subject::Principal(name) => may_hold(kind_of(name).ok_or(format!("no principal {name:?}"))?, &role)?,
                Subject::Group(name) if !groups.contains(name.as_str()) => return Err(format!("no group {name:?}")),
                Subject::Group(_) => {}
            }
        }
        Ok(())
    }

    /// What each principal holds, and until when: its own bindings and its groups'.
    fn index(&self) -> HashMap<String, Vec<(Held, Option<u64>)>> {
        let roles: HashMap<&str, Arc<RoleDef>> = self.roles.iter().filter_map(|r| Some((r.name.as_str(), Arc::new(r.def().ok()?)))).collect();
        let mut held: HashMap<String, Vec<(Held, Option<u64>)>> = HashMap::new();
        for b in &self.bindings {
            let Some(role) = crate::can::builtin(&b.role).or_else(|| roles.get(b.role.as_str())) else { continue };
            let names = match &b.who {
                Subject::Principal(name) => std::slice::from_ref(name),
                Subject::Group(group) => self.group(group).map(|g| g.members.as_slice()).unwrap_or_default(),
            };
            for name in names {
                held.entry(name.clone()).or_default().push((Held { role: Arc::clone(role), scope: b.scope.clone() }, b.expires_at));
            }
        }
        held
    }
}

/// A version as its history lists it: who made it, when, what and why, without its bindings.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(crate) version: u64,
    #[serde(default)]
    pub(crate) at: u64,
    #[serde(default)]
    pub(crate) by: String,
    #[serde(default)]
    pub(crate) what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
}

/// A version of the document and its index: made once per version, so `can` reads what each
/// principal holds and never the document (§7's performance note).
struct Version {
    document: Arc<Document>,
    held: HashMap<String, Vec<(Held, Option<u64>)>>,
}

/// The access document as the server holds it: its newest version, kept where it was read from.
pub(crate) struct Bindings {
    /// None in memory (the tests), where `kept` is the history.
    path: Option<PathBuf>,
    now: RwLock<Version>,
    /// Whether the history may end in a line cut short (the server stopped, or a write failed,
    /// while one was written): the next is put on a line of its own.
    torn: AtomicBool,
    /// Every version, in memory only.
    kept: Mutex<Vec<Document>>,
}

/// Where the access document is kept for the accounts at `accounts`: beside them, under the same
/// name with `access` for `accounts` (`access.json`, `.holon-access.json`).
pub(crate) fn beside(accounts: &Path) -> PathBuf {
    let name = accounts.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let access = if name.contains("accounts") { name.replacen("accounts", "access", 1) } else { format!("{name}.access.json") };
    accounts.with_file_name(access)
}

/// Where the versions of the document at `path` are kept: `access.versions.jsonl`.
pub(crate) fn history(path: &Path) -> PathBuf {
    path.with_extension("versions.jsonl")
}

impl Bindings {
    /// The access document at `path`, as its newest version says: the history's last version when
    /// the server stopped after keeping it there but before writing the document; none yet if
    /// neither is there.
    pub(crate) fn open(path: PathBuf) -> Result<Bindings, String> {
        let read = |p: &Path| match std::fs::read_to_string(p) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("the access file {p:?}: {e}")),
        };
        let mut document = match read(&path)? {
            Some(text) => serde_json::from_str(&text).map_err(|e| format!("the access file {path:?}: {e}"))?,
            None => Document::default(),
        };
        // The last whole line: one cut short was being written when the server stopped, and was
        // never in force.
        let text = read(&history(&path))?.unwrap_or_default();
        let last = text.lines().rev().find_map(|l| serde_json::from_str::<Document>(l).ok());
        if let Some(last) = last.filter(|l| l.version > document.version) {
            document = last;
        }
        let bindings = Bindings::with(Some(path), document);
        bindings.torn.store(!text.is_empty() && !text.ends_with('\n'), Ordering::SeqCst);
        Ok(bindings)
    }

    /// A document kept in memory only (the tests).
    #[cfg(test)]
    pub(crate) fn memory() -> Bindings {
        Bindings::with(None, Document::default())
    }

    fn with(path: Option<PathBuf>, document: Document) -> Bindings {
        let held = document.index();
        Bindings { path, now: RwLock::new(Version { document: Arc::new(document), held }), torn: AtomicBool::new(false), kept: Mutex::new(Vec::new()) }
    }

    /// The newest version.
    pub(crate) fn document(&self) -> Arc<Document> {
        Arc::clone(&self.now.read().unwrap().document)
    }

    /// What `name` holds at `at` (seconds): nothing if nothing is bound to it, and nothing by a
    /// binding expired by then. A request is answered as what its caller held when it came.
    pub(crate) fn held(&self, name: &str, at: u64) -> Arc<[Held]> {
        let now = self.now.read().unwrap();
        let held = now.held.get(name).map(Vec::as_slice).unwrap_or_default();
        held.iter().filter(|(_, until)| until.is_none_or(|until| at < until)).map(|(h, _)| h.clone()).collect()
    }

    /// Keep `document` as the newest version. It is kept once it is in the history, and in force
    /// from then on: written as the document after, so the document is never ahead of its
    /// history, and read from the history at the next start if that write fails. The accounts
    /// book's lock is held (accounts.rs), so versions are kept one at a time.
    pub(crate) fn keep(&self, document: Document) -> Result<(), String> {
        match &self.path {
            Some(path) => {
                let mut line = if self.torn.load(Ordering::SeqCst) { b"\n".to_vec() } else { Vec::new() };
                serde_json::to_writer(&mut line, &document).map_err(|e| e.to_string())?;
                line.push(b'\n');
                let text = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
                let at = history(path);
                let appended = crate::sealing::append_private(&at).and_then(|mut f| f.write_all(&line).and_then(|_| f.sync_data()));
                self.torn.store(appended.is_err(), Ordering::SeqCst);
                appended.map_err(|e| format!("the access history {at:?}: {e}"))?;
                if let Err(e) = crate::sealing::write_private(path, &text) {
                    eprintln!("holon: the access file {path:?}: {e}: version {} is kept in its history, and read from there when the server starts", document.version);
                }
            }
            None => self.kept.lock().unwrap().push(document.clone()),
        }
        let held = document.index();
        *self.now.write().unwrap() = Version { document: Arc::new(document), held };
        Ok(())
    }

    /// Every version kept, as a line of the history (or a document in memory) gives it.
    fn each<T: for<'de> Deserialize<'de>>(&self) -> Result<Vec<T>, String> {
        let Some(path) = &self.path else {
            let kept = self.kept.lock().unwrap();
            return kept.iter().map(|d| serde_json::to_value(d).and_then(serde_json::from_value).map_err(|e| e.to_string())).collect();
        };
        let at = history(path);
        let text = match std::fs::read_to_string(&at) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("the access history {at:?}: {e}")),
        };
        Ok(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
    }

    /// Each version's number, when, by whom, what and why, newest first.
    pub(crate) fn versions(&self) -> Result<Vec<Summary>, String> {
        let mut all: Vec<Summary> = self.each()?;
        all.reverse();
        Ok(all)
    }

    /// Version `n`, from the history.
    pub(crate) fn version(&self, n: u64) -> Result<Option<Document>, String> {
        Ok(self.each::<Document>()?.into_iter().find(|d| d.version == n))
    }
}

/// The words a version says for binding `role` to `who` at `scope`: `bound editor to bob on
/// "payout"`, with when it expires.
pub(crate) fn bound_words(who: &Subject, role: &str, scope: &Scope, expires_at: Option<u64>) -> String {
    let until = expires_at.map(|at| format!(", until {}", crate::cmdline::when(Some(at)))).unwrap_or_default();
    format!("bound {role} to {} {}{until}", who.said(), scope.said())
}

/// The words a version says for unbinding `binding`: `unbound editor from bob on "payout"`.
pub(crate) fn unbound_words(binding: &Binding) -> String {
    format!("unbound {} from {} {}", binding.role, binding.who.said(), binding.scope.said())
}

// ── the routes ────────────────────────────────────────────────────────────────────────────────

/// A binding as the routes answer it: whether it has expired, besides what is kept.
fn listed(b: &Binding, now: u64) -> serde_json::Value {
    let mut v = serde_json::to_value(b).unwrap_or_default();
    v["expired"] = serde_json::json!(!b.live(now));
    v
}

/// What a built-in role is for, as `GET /roles` says it.
fn about(role: Role) -> &'static str {
    match role {
        Role::Viewer => "reads flows, runs and their state, the names of secrets, and the policy",
        Role::Agent => "a viewer's, and proposes: saves its own flows, deploys what needs no approval, calls",
        Role::Editor => "a flow's author and operator: saves, deploys and approves, runs, stops, deletes, decides approvals",
        Role::Admin => "everything, the host's resources and people included",
    }
}

/// A role as `GET /roles` answers it: what it allows, who may hold it, and how many bindings hold it.
fn role_listed(def: &RoleDef, about: Option<&str>, bound: usize) -> serde_json::Value {
    let allows: Vec<&str> = def.allows().into_iter().map(Action::name).collect();
    serde_json::json!({ "name": def.name, "about": about, "allows": allows, "kinds": def.kinds, "builtin": def.builtin.is_some(), "bound": bound })
}

/// Where `who` would be asked `action` for a role allowing it on the host (D19): on the host for
/// the host's actions, and on every flow, those not named yet included, for a flow's (the empty
/// slug, which no binding names, so only the host covers it: can.rs's `everywhere`).
fn on_the_host(action: Action) -> Resource<'static> {
    if Resource::Host.takes(action) { Resource::Host } else { Resource::Flow("") }
}

/// Why `who` may not bind `role` at `scope`, if they may not (the plan's D19): a binding holds
/// something there (400 if not), and gives only what its author holds there (403 if not).
fn may_bind(who: &Caller, role: &Arc<RoleDef>, scope: &Scope) -> Result<(), Response> {
    // What the binding would hold, and on what: each action of the role that the scope covers.
    let slug = match scope {
        Scope::Host => "",
        Scope::Flows(Selector::Slug(slug)) | Scope::Approval { flow: Selector::Slug(slug), .. } => slug.as_str(),
    };
    let held = Held { role: Arc::clone(role), scope: scope.clone() };
    let gives: Vec<(Action, Resource)> = role
        .allows()
        .into_iter()
        .filter_map(|a| {
            let on = match scope {
                Scope::Host => on_the_host(a),
                _ if Resource::Host.takes(a) => Resource::Host,
                Scope::Approval { name, .. } => Resource::Approval { slug, name: Some(name.as_str()) },
                Scope::Flows(_) => Resource::Flow(slug),
            };
            (on.takes(a) && held.covers(&on)).then_some((a, on))
        })
        .collect();
    let name = &role.name;
    if gives.is_empty() {
        return Err(err(400, format!("{name} holds nothing {}: bind a role that holds something there", scope.said())));
    }
    match gives.iter().find(|(a, on)| can(who, *a, on).is_err()) {
        Some((a, _)) => Err(err(403, format!("{name} holds {} {}, which you don't: a binding gives only what its author holds where it is made", a.name(), scope.said()))),
        None => Ok(()),
    }
}

/// `GET /auth/me/can?actions=flow.deploy,run.decide&flow=payout&approval=ok`: for each action,
/// true, or the words its refusal says, as the routes ask it: on the host for the host's actions;
/// on the flow named, or on its approval point for `run.decide` (the one named, or some); with no
/// flow, on every flow at once as the lists ask, or everywhere for what no list asks.
fn me_can(request: &Request, caller: &Caller) -> Response {
    let Some(actions) = request.get_param("actions") else { return err(400, "say which: ?actions=flow.deploy,run.decide") };
    let (flow, point) = (request.get_param("flow"), request.get_param("approval"));
    let mut answer = serde_json::Map::new();
    for name in actions.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let Some(action) = Action::named(name) else { return err(400, format!("{name:?} isn't an action: the catalogue names them as flow.deploy, run.decide…")) };
        let on = match flow.as_deref() {
            _ if Resource::Host.takes(action) => Resource::Host,
            Some(slug) if action == Action::RunDecide => Resource::Approval { slug, name: point.as_deref() },
            Some(slug) => Resource::Flow(slug),
            None if Resource::Flows.takes(action) => Resource::Flows,
            None => Resource::Flow(""),
        };
        answer.insert(name.into(), can(caller, action, &on).map_or_else(|r| r.0.into(), |_| true.into()));
    }
    Response::json(&answer)
}

/// The body a route reads, or 400 saying what it is.
fn body<T: for<'de> Deserialize<'de>>(request: &Request, shape: &str) -> Result<T, Response> {
    read_body(request).and_then(|b| serde_json::from_slice(&b).map_err(|e| err(400, format!("{shape}: {e}"))))
}

/// A change's refusal: 404 for what isn't there (`missing` says what that is), 409 for what is
/// bound or would leave no admin, 400 for the rest.
fn refused(e: String, missing: &str) -> Response {
    let status = if e.starts_with(missing) {
        404
    } else if e.contains("no admin") || e.contains("is bound") {
        409
    } else {
        400
    };
    err(status, e)
}

/// `/bindings…`, `/access…`, `/roles…`, `/groups…` and `/auth/me/can`, or None for any other path
/// (can.rs's table asks who may):
/// - `GET /bindings` (`?who=&role=&flow=`): each binding, with whether it has expired;
/// - `POST /bindings` (`{who: {principal | group}, role, scope, expires_at?, why?}`): the binding,
///   and the version it made;
/// - `DELETE /bindings/<id>` (`?why=`): 204; 409 for the last admin's (D15);
/// - `GET /access/versions`: each version's number, when, by whom, what and why, newest first;
/// - `GET /access` (`?version=n`): that version, whole; the newest without;
/// - `GET /roles`: every role, the built-in ones first, with how many bindings hold it;
/// - `PUT /roles/<name>` (`{about?, allows, kinds, why?}`): an admin's role, defined or changed;
/// - `DELETE /roles/<name>` (`?why=`): 204; 409 while it is bound (D28);
/// - `GET /groups`, `PUT /groups/<name>` (`{about?, members, why?}`: all of them),
///   `DELETE /groups/<name>` (`?why=`), as for roles;
/// - `GET /auth/me/can`: `me_can`.
pub(crate) fn route(state: &crate::AppState, request: &Request, caller: &Caller) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    if url == "/auth/me/can" && method == "GET" {
        return Some(me_can(request, caller));
    }
    let ours = ["/bindings", "/access", "/access/versions", "/roles", "/groups"].contains(&url.as_str()) || ["/bindings/", "/roles/", "/groups/"].iter().any(|p| url.starts_with(p));
    if !ours {
        return None;
    }
    let Some(accounts) = state.access.accounts() else { return Some(err(404, "this server has no accounts")) };
    let by = caller.name.as_deref().unwrap_or("");
    let why = request.get_param("why");
    let now = crate::deploy::epoch_now();
    let document = accounts.access_document();
    let recorded = |what: &str, target: &str| state.activity.record(what, target, Some(by));
    Some(match (method, url.as_str()) {
        ("GET", "/bindings") => {
            let (who, role, flow) = (request.get_param("who"), request.get_param("role"), request.get_param("flow"));
            let kept = document.bindings.iter().filter(|b| {
                let (Subject::Principal(name) | Subject::Group(name)) = &b.who;
                who.as_ref().is_none_or(|w| w == name)
                    && role.as_ref().is_none_or(|r| *r == b.role)
                    && flow.as_ref().is_none_or(|f| match &b.scope {
                        Scope::Host => false,
                        Scope::Flows(s) | Scope::Approval { flow: s, .. } => s.selects(f),
                    })
            });
            Response::json(&serde_json::json!({ "version": document.version, "bindings": kept.map(|b| listed(b, now)).collect::<Vec<_>>() }))
        }
        ("POST", "/bindings") => {
            #[derive(Deserialize)]
            struct New {
                who: Subject,
                role: String,
                scope: Scope,
                #[serde(default)]
                expires_at: Option<u64>,
                #[serde(default)]
                why: Option<String>,
            }
            let shape = "a binding is {\"who\": {\"principal\": \"bob\"}, \"role\": \"editor\", \"scope\": {\"flows\": {\"slug\": \"payout\"}}}";
            let new: New = match body(request, shape) {
                Ok(new) => new,
                Err(r) => return Some(r),
            };
            let Some(role) = document.role(&new.role) else { return Some(err(400, format!("no role {:?}: GET /roles lists them", new.role))) };
            if let Err(r) = new.scope.check().map_err(|e| err(400, e)).and_then(|_| may_bind(caller, &role, &new.scope)) {
                return Some(r);
            }
            if new.expires_at.is_some_and(|at| at <= now) {
                return Some(err(400, "expires_at has passed: a binding expires later than now"));
            }
            match accounts.bind(new.who, &new.role, new.scope, new.expires_at, by, new.why.as_deref()) {
                Ok(binding) => {
                    recorded(&format!("bound {} to", binding.role), &format!("{} {}", binding.who.said(), binding.scope.said()));
                    Response::json(&serde_json::json!({ "binding": listed(&binding, now), "version": accounts.access_document().version }))
                }
                Err(e) => err(400, e),
            }
        }
        ("GET", "/access/versions") => match accounts.access_versions() {
            Ok(versions) => Response::json(&serde_json::json!({ "versions": versions })),
            Err(e) => err(500, e),
        },
        ("GET", "/access") => match request.get_param("version") {
            None => Response::json(&*document),
            Some(n) => match n.parse::<u64>().map_err(|_| err(400, "version is a number")).and_then(|n| accounts.access_version(n).map_err(|e| err(500, e))) {
                Ok(Some(document)) => Response::json(&document),
                Ok(None) => err(404, format!("no version {n}")),
                Err(r) => r,
            },
        },
        ("GET", "/roles") => {
            let builtin = crate::can::BUILTIN.iter().map(|d| role_listed(d, d.builtin.map(about), document.bound(Some(&d.name), None)));
            let custom = document.roles.iter().filter_map(|r| Some(role_listed(&r.def().ok()?, r.about.as_deref(), document.bound(Some(&r.name), None))));
            Response::json(&serde_json::json!({ "version": document.version, "roles": builtin.chain(custom).collect::<Vec<_>>() }))
        }
        ("GET", "/groups") => {
            let groups = document.groups.iter().map(|g| serde_json::json!({ "name": g.name, "about": g.about, "members": g.members, "bound": document.bound(None, Some(&g.name)) }));
            Response::json(&serde_json::json!({ "version": document.version, "groups": groups.collect::<Vec<_>>() }))
        }
        _ => {
            if let Some(id) = url.strip_prefix("/bindings/")
                && method == "DELETE"
            {
                let Ok(id) = id.parse::<u64>() else { return Some(err(404, format!("no binding {id}"))) };
                return Some(match accounts.unbind(id, by, why.as_deref()) {
                    Ok(binding) => {
                        recorded(&format!("unbound {} from", binding.role), &format!("{} {}", binding.who.said(), binding.scope.said()));
                        Response::empty_204()
                    }
                    Err(e) => refused(e, "no binding"),
                });
            }
            if let Some(name) = url.strip_prefix("/roles/") {
                return Some(match method {
                    "PUT" => define(state, request, caller, accounts, name),
                    "DELETE" => match accounts.remove_role(name, by, why.as_deref()) {
                        Ok(()) => {
                            recorded("removed the role", name);
                            Response::empty_204()
                        }
                        Err(e) => refused(e, "no role"),
                    },
                    _ => Response::empty_404(),
                });
            }
            if let Some(name) = url.strip_prefix("/groups/") {
                return Some(match method {
                    "PUT" => make_group(state, request, caller, accounts, name),
                    "DELETE" => match accounts.remove_group(name, by, why.as_deref()) {
                        Ok(()) => {
                            recorded("removed the group", name);
                            Response::empty_204()
                        }
                        Err(e) => refused(e, "no group"),
                    },
                    _ => Response::empty_404(),
                });
            }
            Response::empty_404()
        }
    })
}

/// `PUT /roles/<name>`: an admin's role, defined or changed. It allows only what its author holds
/// on the host (D19: 403 if not); a role still bound changes for every binding of it, refused
/// when one is to a principal whose kind it no longer allows.
fn define(state: &crate::AppState, request: &Request, caller: &Caller, accounts: &crate::accounts::Accounts, name: &str) -> Response {
    #[derive(Deserialize)]
    struct Def {
        #[serde(default)]
        about: Option<String>,
        allows: Vec<String>,
        kinds: Vec<Kind>,
        #[serde(default)]
        why: Option<String>,
    }
    let def: Def = match body(request, "a role is {\"allows\": [\"run.decide\", \"run.view\"], \"kinds\": [\"person\"]}") {
        Ok(def) => def,
        Err(r) => return r,
    };
    let about = def.about.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
    let text = RoleText { name: name.into(), about, allows: def.allows, kinds: def.kinds };
    let role = match text.def() {
        Ok(role) => role,
        Err(e) => return err(400, e),
    };
    if let Some(a) = role.allows().into_iter().find(|a| can(caller, *a, &on_the_host(*a)).is_err()) {
        return err(403, format!("{name} would allow {}, which you don't hold on this host: a role allows only what its author holds", a.name()));
    }
    let by = caller.name.as_deref().unwrap_or("");
    match accounts.define_role(text, by, def.why.as_deref()) {
        Ok(changed) => {
            state.activity.record(if changed { "changed the role" } else { "defined the role" }, name, Some(by));
            let document = accounts.access_document();
            let about = document.roles.iter().find(|r| r.name == name).and_then(|r| r.about.clone());
            Response::json(&serde_json::json!({ "role": role_listed(&role, about.as_deref(), document.bound(Some(name), None)), "version": document.version }))
        }
        Err(e) => err(400, e),
    }
}

/// `PUT /groups/<name>`: a group, made or changed, with all its members.
fn make_group(state: &crate::AppState, request: &Request, caller: &Caller, accounts: &crate::accounts::Accounts, name: &str) -> Response {
    #[derive(Deserialize)]
    struct Def {
        #[serde(default)]
        about: Option<String>,
        members: Vec<String>,
        #[serde(default)]
        why: Option<String>,
    }
    let def: Def = match body(request, "a group is {\"members\": [\"bob\", \"cy\"]}") {
        Ok(def) => def,
        Err(r) => return r,
    };
    let mut members = def.members;
    members.sort();
    members.dedup();
    let about = def.about.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
    let group = Group { name: name.into(), about, members };
    let by = caller.name.as_deref().unwrap_or("");
    match accounts.set_group(group.clone(), by, def.why.as_deref()) {
        Ok(changed) => {
            state.activity.record(if changed { "changed the group" } else { "made the group" }, name, Some(by));
            let document = accounts.access_document();
            Response::json(&serde_json::json!({ "group": group, "bound": document.bound(None, Some(name)), "version": document.version }))
        }
        Err(e) => err(400, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-bindings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn next(document: &Document, by: &str, what: &str, f: impl FnOnce(&mut Document)) -> Document {
        let mut d = document.clone();
        f(&mut d);
        d.after(document, by, what, None, 1_800_000_000 + document.version)
    }

    fn at(role: Role, scope: Scope) -> Held {
        Held { role: Arc::clone(crate::can::role(role)), scope }
    }

    fn at_host(role: Role) -> Held {
        at(role, Scope::Host)
    }

    /// Each version is kept with who made it, when and what it changed; the newest is read back,
    /// and every one is in the history as it was.
    #[test]
    fn a_version_is_kept_with_who_when_and_what_and_read_back() {
        let path = dir("kept").join("access.json");
        let access = Bindings::open(path.clone()).unwrap();
        assert_eq!(*access.document(), Document::default(), "none yet");
        let one = next(&access.document(), "ada", "bound admin to ada on this host", |d| d.set_host_role("ada", Role::Admin, "ada", 1));
        access.keep(one.clone()).unwrap();
        let two = next(&access.document(), "ada", "bound editor to bob on this host", |d| d.set_host_role("bob", Role::Editor, "ada", 2));
        access.keep(two.clone()).unwrap();
        let three = next(&access.document(), "ada", "bound viewer to bob on this host", |d| d.set_host_role("bob", Role::Viewer, "ada", 3));
        access.keep(three.clone()).unwrap();
        assert_eq!(&*access.held("bob", 4), [at_host(Role::Viewer)], "in place of editor");
        assert_eq!(three.bindings.iter().map(|b| b.id).collect::<Vec<_>>(), [0, 2], "ids aren't reused");
        let mut same = three.clone();
        same.set_host_role("bob", Role::Viewer, "ada", 4);
        assert_eq!(same, three, "the role bound there already: nothing changes");

        let again = Bindings::open(path).unwrap();
        assert_eq!(*again.document(), three);
        assert_eq!([1, 2, 3].map(|n| again.version(n).unwrap()), [Some(one), Some(two), Some(three)]);
        assert_eq!(again.versions().unwrap().iter().map(|v| (v.version, v.what.as_str())).collect::<Vec<_>>(), [
            (3, "bound viewer to bob on this host"),
            (2, "bound editor to bob on this host"),
            (1, "bound admin to ada on this host"),
        ]);
        assert_eq!(&*again.held("ada", 4), [at_host(Role::Admin)]);
        assert!(again.held("cy", 4).is_empty());
    }

    /// A version kept in the history but not yet written as the document (the server stopped
    /// between the two) is the newest; a last line cut short was never in force.
    #[test]
    fn the_history_is_never_behind_the_document() {
        let path = dir("history").join(".holon-access.json");
        let access = Bindings::open(path.clone()).unwrap();
        let one = next(&access.document(), "ada", "bound admin to ada on this host", |d| d.set_host_role("ada", Role::Admin, "ada", 1));
        access.keep(one.clone()).unwrap();
        let two = next(&one, "ada", "bound editor to bob on this host", |d| d.set_host_role("bob", Role::Editor, "ada", 2));
        let mut line = serde_json::to_string(&two).unwrap();
        line.push('\n');
        std::fs::OpenOptions::new().append(true).open(history(&path)).unwrap().write_all(line.as_bytes()).unwrap();
        assert_eq!(*Bindings::open(path.clone()).unwrap().document(), two, "the history's");
        std::fs::OpenOptions::new().append(true).open(history(&path)).unwrap().write_all(b"{\"version\":3,\"by\":\"ad").unwrap();
        let access = Bindings::open(path.clone()).unwrap();
        assert_eq!(*access.document(), two, "not the line cut short");
        // The next version is on a line of its own, and read back.
        let three = next(&two, "ada", "bound viewer to cy on this host", |d| d.set_host_role("cy", Role::Viewer, "ada", 3));
        access.keep(three.clone()).unwrap();
        assert_eq!(*Bindings::open(path.clone()).unwrap().document(), three);
        assert_eq!(access.version(3).unwrap(), Some(three));
    }

    /// A binding that expires holds until then, and nothing after; it stays in the document, and
    /// a request is answered as its caller held when it came.
    #[test]
    fn a_binding_holds_until_it_expires() {
        let access = Bindings::memory();
        let mut d = Document::default();
        let flow = Scope::Flows(Selector::Slug("payout".into()));
        d.bind(Subject::Principal("bob".into()), "editor", flow.clone(), Some(100), "ada", 1);
        d.bind(Subject::Principal("bob".into()), "viewer", Scope::Host, None, "ada", 1);
        access.keep(d.after(&Document::default(), "ada", "bound", None, 1)).unwrap();
        assert_eq!(&*access.held("bob", 99), [at(Role::Editor, flow), at_host(Role::Viewer)]);
        assert_eq!(&*access.held("bob", 100), [at_host(Role::Viewer)], "expired at 100");
        assert_eq!(access.document().bindings.len(), 2, "kept until unbound");
    }

    /// A binding to a group holds for each member, as one to them would; an admin's role is read
    /// as defined; a member taken out of the group, or forgotten, holds nothing by it.
    #[test]
    fn a_groups_binding_holds_for_each_member() {
        let access = Bindings::memory();
        let mut d = Document::default();
        d.roles.push(RoleText { name: "payout-approver".into(), about: None, allows: vec!["run.decide".into(), "run.view".into()], kinds: vec![Kind::Person] });
        d.groups.push(Group { name: "payments".into(), about: None, members: vec!["bob".into(), "cy".into()] });
        let flow = Scope::Flows(Selector::Slug("payout".into()));
        d.bind(Subject::Group("payments".into()), "payout-approver", flow.clone(), None, "ada", 1);
        access.keep(d.clone().after(&Document::default(), "ada", "bound", None, 1)).unwrap();
        let approver = Arc::new(RoleDef::custom("payout-approver", &[Action::RunView, Action::RunDecide], &[Kind::Person]));
        for name in ["bob", "cy"] {
            assert_eq!(&*access.held(name, 2), [Held { role: Arc::clone(&approver), scope: flow.clone() }], "{name}");
        }
        assert_eq!(access.document().reaching("bob").map(|(b, g)| (b.id, g)).collect::<Vec<_>>(), [(0, Some("payments"))]);
        d.forget("cy");
        access.keep(d.after(&access.document(), "ada", "removed cy", None, 2)).unwrap();
        assert!(access.held("cy", 3).is_empty());
        assert_eq!(access.held("bob", 3).len(), 1);
    }

    /// What the server keeps is checked whole: an admin's role is a role (named otherwise than a
    /// built-in one, allowing actions the catalogue names, held by some kind), defined once; a
    /// group's members are principals here; a binding is of a role here, to a group here or a
    /// principal whose kind may hold it.
    #[test]
    fn what_is_kept_is_checked_whole() {
        let here = |name: &str| match name {
            "bob" => Some(Kind::Person),
            "claude" => Some(Kind::Agent),
            _ => None,
        };
        let role = |name: &str, allows: &[&str], kinds: &[Kind]| RoleText { name: name.into(), about: None, allows: allows.iter().map(|a| a.to_string()).collect(), kinds: kinds.to_vec() };
        let refused = |f: &dyn Fn(&mut Document)| {
            let mut d = Document::default();
            f(&mut d);
            d.check(here).unwrap_err()
        };
        assert_eq!(refused(&|d| d.roles.push(role("editor", &["run.view"], &[Kind::Person]))), "editor is a built-in role: name yours otherwise");
        assert_eq!(refused(&|d| d.roles.push(role("r", &["run.fly"], &[Kind::Person]))), "\"run.fly\" isn't an action: the catalogue names them as flow.deploy, run.decide…");
        assert_eq!(refused(&|d| d.roles.push(role("r", &[], &[Kind::Person]))), "r allows nothing: give it an action");
        assert_eq!(refused(&|d| d.roles.push(role("r", &["run.view"], &[]))), "no kind may hold r: give it person, agent or service");
        assert_eq!(refused(&|d| d.roles.extend([role("r", &["run.view"], &[Kind::Person]), role("r", &["run.view"], &[Kind::Person])])), "the role r is defined twice");
        assert_eq!(refused(&|d| d.groups.push(Group { name: "g".into(), about: None, members: vec!["nobody".into()] })), "no principal \"nobody\": a group's members are principals here");
        assert_eq!(refused(&|d| { d.bind(Subject::Principal("bob".into()), "nope", Scope::Host, None, "ada", 1); }), "no role \"nope\"");
        assert_eq!(refused(&|d| { d.bind(Subject::Group("g".into()), "viewer", Scope::Host, None, "ada", 1); }), "no group \"g\"");
        assert_eq!(refused(&|d| { d.bind(Subject::Principal("claude".into()), "editor", Scope::Host, None, "ada", 1); }), "an agent is an agent or a viewer");
        assert_eq!(
            refused(&|d| {
                d.roles.push(role("r", &["run.view"], &[Kind::Person]));
                d.bind(Subject::Principal("claude".into()), "r", Scope::Host, None, "ada", 1);
            }),
            "an agent may not hold r: it is held by a person"
        );
        let mut fine = Document::default();
        fine.roles.push(role("r", &["run.view"], &[Kind::Agent, Kind::Person]));
        fine.groups.push(Group { name: "g".into(), about: None, members: vec!["bob".into(), "claude".into()] });
        fine.bind(Subject::Group("g".into()), "editor", Scope::Host, None, "ada", 1);
        fine.bind(Subject::Principal("claude".into()), "r", Scope::Host, None, "ada", 1);
        assert_eq!(fine.check(here), Ok(()), "a group's binding holds for the kinds its role allows, and nothing for the others");
    }

    /// A binding holds something where it is made, and gives only what its author holds there
    /// (the plan's D19): an admin binds every role where it holds something, which is anywhere
    /// but a viewer or an agent at an approval point; an editor binds no admin; and an admin of
    /// one flow binds at that flow only.
    #[test]
    fn a_binding_gives_only_what_its_author_holds_there() {
        let by = |held: Vec<Held>| Caller { name: Some("ada".into()), kind: Some(Kind::Person), holds: Some(held.into()) };
        let status = |r: Result<(), Response>| r.err().map(|r| r.status_code);
        let (d, e) = (Scope::Flows(Selector::Slug("d".into())), Scope::Flows(Selector::Slug("e".into())));
        let ok = Scope::Approval { flow: Selector::Slug("d".into()), name: "ok".into() };
        let r = |role: Role| crate::can::role(role);
        let admin = by(vec![at_host(Role::Admin)]);
        for scope in [Scope::Host, d.clone(), ok.clone()] {
            for role in [Role::Viewer, Role::Agent, Role::Editor, Role::Admin] {
                let nothing = matches!((role, &scope), (Role::Viewer | Role::Agent, Scope::Approval { .. }));
                assert_eq!(status(may_bind(&admin, r(role), &scope)), nothing.then_some(400), "{} {}", role.name(), scope.said());
            }
        }
        let editor = by(vec![at_host(Role::Editor)]);
        assert_eq!(status(may_bind(&editor, r(Role::Editor), &Scope::Host)), None);
        assert_eq!(status(may_bind(&editor, r(Role::Editor), &ok)), None);
        assert_eq!(status(may_bind(&editor, r(Role::Admin), &Scope::Host)), Some(403));
        assert_eq!(status(may_bind(&editor, r(Role::Admin), &d)), Some(403), "secret.set on d");
        let admin_of_d = by(vec![at(Role::Admin, d.clone())]);
        assert_eq!(status(may_bind(&admin_of_d, r(Role::Editor), &d)), None);
        assert_eq!(status(may_bind(&admin_of_d, r(Role::Editor), &ok)), None);
        assert_eq!(status(may_bind(&admin_of_d, r(Role::Editor), &e)), Some(403));
        assert_eq!(status(may_bind(&admin_of_d, r(Role::Viewer), &Scope::Host)), Some(403));
        // An admin's role holding only the host's actions holds nothing at a flow.
        let people = Arc::new(RoleDef::custom("people-viewer", &[Action::PeopleView], &[Kind::Person]));
        assert_eq!(status(may_bind(&admin, &people, &d)), Some(400));
        assert_eq!(status(may_bind(&admin, &people, &Scope::Host)), None);
    }

    #[test]
    fn a_scope_is_kept_and_said_as_written() {
        let scopes = [
            (Scope::Host, serde_json::json!("host"), "on this host"),
            (Scope::Flows(Selector::Slug("payout".into())), serde_json::json!({ "flows": { "slug": "payout" } }), "on \"payout\""),
            (Scope::Approval { flow: Selector::Slug("payout".into()), name: "ok".into() }, serde_json::json!({ "approval": { "flow": { "slug": "payout" }, "name": "ok" } }), "for \"payout\"'s approval \"ok\""),
        ];
        for (scope, kept, said) in scopes {
            assert_eq!(serde_json::to_value(&scope).unwrap(), kept);
            assert_eq!(serde_json::from_value::<Scope>(kept).unwrap(), scope);
            assert_eq!(scope.said(), said);
        }
        assert!(Scope::Flows(Selector::Slug("a b".into())).check().is_err());
        assert!(Scope::Approval { flow: Selector::Slug("payout".into()), name: String::new() }.check().is_err());
    }

    #[test]
    fn the_document_is_kept_beside_the_accounts() {
        let at = |p: &str| beside(Path::new(p)).to_string_lossy().replace('\\', "/");
        assert_eq!(at("/data/accounts.json"), "/data/access.json");
        assert_eq!(at("/srv/.holon-accounts.json"), "/srv/.holon-access.json");
        assert_eq!(at("/srv/people.json"), "/srv/people.json.access.json");
        assert_eq!(history(Path::new("/data/access.json")).to_string_lossy().replace('\\', "/"), "/data/access.versions.jsonl");
    }
}
