//! What each principal holds, and where (docs/L4s/access-and-approvals-L4.md §5.3 and §6):
//! bindings of a role to a principal at a scope, kept in one document, `access.json` beside the
//! accounts file (`.holon-access.json` beside `.holon-accounts.json`). Every change makes a new
//! version, with who made it, when, what it changed and why, and every version is kept for good,
//! one per line, in `access.versions.jsonl` beside it.
//!
//! A scope is the host (everything), a flow by its slug (what is of that flow, and the lists,
//! which keep the flows their caller may see), or one approval point of a flow (deciding it, and
//! nothing else). A binding may expire: from then on it holds nothing, and stays in the document
//! until it is unbound.
//!
//! The accounts book (accounts.rs) keeps who the principals are, and owns this document: a change
//! to both, as inviting someone with a role, is made under the book's lock and written so that no
//! binding on disk names a principal the book doesn't have (accounts.rs `change_access`). The
//! routes that read and change it are at the end of this file.
//!
//! A binding is of one of the built-in roles (can.rs). Groups and custom roles come next
//! (docs/L4s/roles-and-bindings-plan.md, step 3b), and scopes by tag after (3d).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rouille::{Request, Response};
use serde::{Deserialize, Serialize};

use crate::access::{Caller, Role};
use crate::can::{can, Action, Resource};
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
    #[serde(default)]
    pub(crate) bindings: Vec<Binding>,
    /// The next binding's id: ids are never reused, so an old version's ids name what they named.
    #[serde(default)]
    next_id: u64,
}

/// A role bound to a principal at a scope, until it expires if it does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub(crate) id: u64,
    pub(crate) who: Subject,
    pub(crate) role: Role,
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

/// Whom a binding is to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Subject {
    Principal(String),
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
    pub(crate) role: Role,
    pub(crate) scope: Scope,
}

impl Held {
    /// What the OS user's run file holds (local.rs): admin, on the host, while the server runs.
    /// It is never in the document.
    pub(crate) fn admin() -> Arc<[Held]> {
        Arc::from([Held { role: Role::Admin, scope: Scope::Host }])
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

/// The widest built-in role of `held` on the host (admin, editor, agent, then viewer): what the
/// editor shows a principal as, until it asks `can` what it may do (the plan's step 3c).
pub(crate) fn host_role(held: &[Held]) -> Option<Role> {
    let rank = |r: &Role| match r {
        Role::Admin => 3,
        Role::Editor => 2,
        Role::Agent => 1,
        Role::Viewer => 0,
    };
    held.iter().filter(|h| h.scope == Scope::Host).map(|h| h.role).max_by_key(rank)
}

impl Document {
    /// `name`'s bindings.
    pub(crate) fn of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Binding> + 'a {
        self.bindings.iter().filter(move |b| matches!(&b.who, Subject::Principal(n) if n == name))
    }

    /// This, as the version after `before`: made by `by` at `at`, saying `what` and `why`.
    pub(crate) fn after(self, before: &Document, by: &str, what: &str, why: Option<&str>, at: u64) -> Document {
        Document { version: before.version + 1, at, by: by.into(), what: what.into(), why: why.map(String::from), ..self }
    }

    /// Bind `role` to `name` at `scope`, until `expires_at` if given: the new binding's id.
    pub(crate) fn bind(&mut self, name: &str, role: Role, scope: Scope, expires_at: Option<u64>, by: &str, at: u64) -> u64 {
        let id = self.next_id;
        self.bindings.push(Binding { id, who: Subject::Principal(name.into()), role, scope, by: by.into(), at, expires_at });
        self.next_id += 1;
        id
    }

    /// Bind `role` to `name` at the host, in place of the roles bound to it there (none changes
    /// if that is the one bound there already).
    pub(crate) fn set_host_role(&mut self, name: &str, role: Role, by: &str, at: u64) {
        let host = |b: &Binding| matches!(&b.who, Subject::Principal(n) if n == name) && b.scope == Scope::Host;
        if self.bindings.iter().filter(|b| host(b)).map(|b| (b.role, b.expires_at)).eq([(role, None)]) {
            return;
        }
        self.bindings.retain(|b| !host(b));
        self.bind(name, role, Scope::Host, None, by, at);
    }

    /// Unbind the binding `id`: it, if it was there.
    pub(crate) fn unbind(&mut self, id: u64) -> Option<Binding> {
        let at = self.bindings.iter().position(|b| b.id == id)?;
        Some(self.bindings.remove(at))
    }

    /// Unbind everything bound to `name`.
    pub(crate) fn forget(&mut self, name: &str) {
        self.bindings.retain(|b| !matches!(&b.who, Subject::Principal(n) if n == name));
    }

    /// What each principal holds, and until when.
    fn index(&self) -> HashMap<String, Vec<(Held, Option<u64>)>> {
        let mut held: HashMap<String, Vec<(Held, Option<u64>)>> = HashMap::new();
        for b in &self.bindings {
            let Subject::Principal(name) = &b.who;
            held.entry(name.clone()).or_default().push((Held { role: b.role, scope: b.scope.clone() }, b.expires_at));
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

// ── the routes ────────────────────────────────────────────────────────────────────────────────

/// A binding as the routes answer it: whether it has expired, besides what is kept.
fn listed(b: &Binding, now: u64) -> serde_json::Value {
    let mut v = serde_json::to_value(b).unwrap_or_default();
    v["expired"] = serde_json::json!(!b.live(now));
    v
}

/// Why `who` may not bind `role` at `scope`, if they may not (the plan's D19): a binding holds
/// something there (400 if not), and gives only what its author holds there (403 if not).
fn may_bind(who: &Caller, role: Role, scope: &Scope) -> Result<(), Response> {
    let def = crate::can::role(role);
    // What the binding would hold, and on what: each action of the role that the scope covers.
    let slug = match scope {
        Scope::Host => "",
        Scope::Flows(Selector::Slug(slug)) | Scope::Approval { flow: Selector::Slug(slug), .. } => slug.as_str(),
    };
    let held = Held { role, scope: scope.clone() };
    let gives: Vec<(Action, Resource)> = Action::ALL
        .into_iter()
        .filter(|a| def.holds_any(*a))
        .filter_map(|a| {
            let on = match scope {
                _ if Resource::Host.takes(a) => Resource::Host,
                Scope::Approval { name, .. } => Resource::Approval { slug, name: Some(name.as_str()) },
                // The flow; at the host, the empty slug, which no binding names, so only the host
                // covers it (can.rs's `everywhere`).
                _ => Resource::Flow(slug),
            };
            (on.takes(a) && held.covers(&on)).then_some((a, on))
        })
        .collect();
    let role = role.name();
    if gives.is_empty() {
        return Err(err(400, format!("{role} holds nothing {}: bind a role that holds something there", scope.said())));
    }
    match gives.iter().find(|(a, on)| can(who, *a, on).is_err()) {
        Some((a, _)) => Err(err(403, format!("{role} holds {} {}, which you don't: a binding gives only what its author holds where it is made", a.name(), scope.said()))),
        None => Ok(()),
    }
}

/// `/bindings…` and `/access…`, or None for any other path (can.rs's table asks who may):
/// - `GET /bindings` (`?who=&role=&flow=`): each binding, with whether it has expired;
/// - `POST /bindings` (`{who: {principal}, role, scope, expires_at?, why?}`): the binding, and the
///   version it made;
/// - `DELETE /bindings/<id>` (`?why=`): 204; 409 for the last admin's (D15);
/// - `GET /access/versions`: each version's number, when, by whom, what and why, newest first;
/// - `GET /access` (`?version=n`): that version, whole; the newest without.
pub(crate) fn route(state: &crate::AppState, request: &Request, caller: &Caller) -> Option<Response> {
    let url = request.url();
    if url != "/bindings" && !url.starts_with("/bindings/") && url != "/access" && url != "/access/versions" {
        return None;
    }
    let Some(accounts) = state.access.accounts() else { return Some(err(404, "this server has no accounts")) };
    let by = caller.name.as_deref().unwrap_or("");
    let now = crate::deploy::epoch_now();
    let method = request.method();
    Some(match (method, url.as_str()) {
        ("GET", "/bindings") => {
            let (who, role, flow) = (request.get_param("who"), request.get_param("role"), request.get_param("flow"));
            let document = accounts.access_document();
            let kept = document.bindings.iter().filter(|b| {
                let Subject::Principal(name) = &b.who;
                who.as_ref().is_none_or(|w| w == name)
                    && role.as_ref().is_none_or(|r| r == b.role.name())
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
                role: Role,
                scope: Scope,
                #[serde(default)]
                expires_at: Option<u64>,
                #[serde(default)]
                why: Option<String>,
            }
            let new: New = match read_body(request).and_then(|b| serde_json::from_slice(&b).map_err(|e| err(400, format!("a binding is {{\"who\": {{\"principal\": \"bob\"}}, \"role\": \"editor\", \"scope\": {{\"flows\": {{\"slug\": \"payout\"}}}}}}: {e}")))) {
                Ok(new) => new,
                Err(r) => return Some(r),
            };
            let Subject::Principal(name) = &new.who;
            if let Err(r) = new.scope.check().map_err(|e| err(400, e)).and_then(|_| may_bind(caller, new.role, &new.scope)) {
                return Some(r);
            }
            if new.expires_at.is_some_and(|at| at <= now) {
                return Some(err(400, "expires_at has passed: a binding expires later than now"));
            }
            match accounts.bind(name, new.role, new.scope, new.expires_at, by, new.why.as_deref()) {
                Ok(binding) => {
                    state.activity.record(&format!("bound {} to", binding.role.name()), &format!("{name} {}", binding.scope.said()), Some(by));
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
            None => Response::json(&*accounts.access_document()),
            Some(n) => match n.parse::<u64>().map_err(|_| err(400, "version is a number")).and_then(|n| accounts.access_version(n).map_err(|e| err(500, e))) {
                Ok(Some(document)) => Response::json(&document),
                Ok(None) => err(404, format!("no version {n}")),
                Err(r) => r,
            },
        },
        _ => {
            let Some(id) = url.strip_prefix("/bindings/").filter(|_| method == "DELETE") else { return Some(Response::empty_404()) };
            let Ok(id) = id.parse::<u64>() else { return Some(err(404, format!("no binding {id}"))) };
            let why = request.get_param("why");
            match accounts.unbind(id, by, why.as_deref()) {
                Ok(binding) => {
                    let Subject::Principal(name) = &binding.who;
                    state.activity.record(&format!("unbound {} from", binding.role.name()), &format!("{name} {}", binding.scope.said()), Some(by));
                    Response::empty_204()
                }
                Err(e) if e.starts_with("no binding") => err(404, e),
                Err(e) if e.contains("no admin") => err(409, e),
                Err(e) => err(400, e),
            }
        }
    })
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

    fn at_host(role: Role) -> Held {
        Held { role, scope: Scope::Host }
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
        d.bind("bob", Role::Editor, flow.clone(), Some(100), "ada", 1);
        d.bind("bob", Role::Viewer, Scope::Host, None, "ada", 1);
        access.keep(d.after(&Document::default(), "ada", "bound", None, 1)).unwrap();
        assert_eq!(&*access.held("bob", 99), [Held { role: Role::Editor, scope: flow }, at_host(Role::Viewer)]);
        assert_eq!(&*access.held("bob", 100), [at_host(Role::Viewer)], "expired at 100");
        assert_eq!(access.document().bindings.len(), 2, "kept until unbound");
    }

    /// A binding holds something where it is made, and gives only what its author holds there
    /// (the plan's D19): an admin binds every role where it holds something, which is anywhere
    /// but a viewer or an agent at an approval point; an editor binds no admin; and an admin of
    /// one flow binds at that flow only.
    #[test]
    fn a_binding_gives_only_what_its_author_holds_there() {
        let by = |held: Vec<Held>| Caller { name: Some("ada".into()), kind: Some(crate::accounts::Kind::Person), holds: Some(held.into()) };
        let status = |r: Result<(), Response>| r.err().map(|r| r.status_code);
        let (d, e) = (Scope::Flows(Selector::Slug("d".into())), Scope::Flows(Selector::Slug("e".into())));
        let ok = Scope::Approval { flow: Selector::Slug("d".into()), name: "ok".into() };
        let admin = by(vec![at_host(Role::Admin)]);
        for scope in [Scope::Host, d.clone(), ok.clone()] {
            for role in [Role::Viewer, Role::Agent, Role::Editor, Role::Admin] {
                let nothing = matches!((role, &scope), (Role::Viewer | Role::Agent, Scope::Approval { .. }));
                assert_eq!(status(may_bind(&admin, role, &scope)), nothing.then_some(400), "{} {}", role.name(), scope.said());
            }
        }
        let editor = by(vec![at_host(Role::Editor)]);
        assert_eq!(status(may_bind(&editor, Role::Editor, &Scope::Host)), None);
        assert_eq!(status(may_bind(&editor, Role::Editor, &ok)), None);
        assert_eq!(status(may_bind(&editor, Role::Admin, &Scope::Host)), Some(403));
        assert_eq!(status(may_bind(&editor, Role::Admin, &d)), Some(403), "secret.set on d");
        let admin_of_d = by(vec![Held { role: Role::Admin, scope: d.clone() }]);
        assert_eq!(status(may_bind(&admin_of_d, Role::Editor, &d)), None);
        assert_eq!(status(may_bind(&admin_of_d, Role::Editor, &ok)), None);
        assert_eq!(status(may_bind(&admin_of_d, Role::Editor, &e)), Some(403));
        assert_eq!(status(may_bind(&admin_of_d, Role::Viewer, &Scope::Host)), Some(403));
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
