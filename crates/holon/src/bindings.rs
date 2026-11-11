//! What each principal holds, and where (docs/L4s/access-and-approvals-L4.md §5.3 and §6):
//! bindings of a role to a principal at a scope, kept in one document, `access.json` beside the
//! accounts file (`.holon-access.json` beside `.holon-accounts.json`). Every change makes a new
//! version, with who made it, when and what it changed, and every version is kept for good, one
//! per line, in `access.versions.jsonl` beside it.
//!
//! The accounts book (accounts.rs) keeps who the principals are, and owns this document: a change
//! to both, as inviting someone with a role, is made under the book's lock and written so that no
//! binding on disk names a principal the book doesn't have (accounts.rs `change_access`).
//!
//! Today a binding is one of the built-in roles (can.rs) bound to a principal at the host: what
//! each principal's `role` was before. Custom roles, groups, and scopes at a flow or an approval
//! point come next (docs/L4s/roles-and-bindings-plan.md, step 3b).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::access::Role;

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
    #[serde(default)]
    pub(crate) bindings: Vec<Binding>,
    /// The next binding's id: ids are never reused, so an old version's ids name what they named.
    #[serde(default)]
    next_id: u64,
}

/// A role bound to a principal at a scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub(crate) id: u64,
    pub(crate) who: Subject,
    pub(crate) role: Role,
    pub(crate) scope: Scope,
    pub(crate) by: String,
    pub(crate) at: u64,
}

/// Whom a binding is to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Subject {
    Principal(String),
}

/// Where a binding holds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Scope {
    /// Everywhere: the host, every flow, every approval.
    Host,
}

/// A binding as `can` reads it: the role, and where.
#[derive(Clone, Copy, Debug, PartialEq)]
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

    /// Whether it holds on `on`: the host's scope covers everything.
    pub(crate) fn covers(&self, _on: &crate::can::Resource<'_>) -> bool {
        match self.scope {
            Scope::Host => true,
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

    /// This, as the version after `before`: made by `by` at `at`, saying `what`.
    pub(crate) fn after(self, before: &Document, by: &str, what: &str, at: u64) -> Document {
        Document { version: before.version + 1, at, by: by.into(), what: what.into(), ..self }
    }

    /// Bind `role` to `name` at the host, in place of the roles bound to it there (none changes
    /// if that is the one bound there already).
    pub(crate) fn set_host_role(&mut self, name: &str, role: Role, by: &str, at: u64) {
        if self.of(name).filter(|b| b.scope == Scope::Host).map(|b| b.role).eq([role]) {
            return;
        }
        self.bindings.retain(|b| !(matches!(&b.who, Subject::Principal(n) if n == name) && b.scope == Scope::Host));
        self.bindings.push(Binding { id: self.next_id, who: Subject::Principal(name.into()), role, scope: Scope::Host, by: by.into(), at });
        self.next_id += 1;
    }

    /// Unbind everything bound to `name`.
    pub(crate) fn forget(&mut self, name: &str) {
        self.bindings.retain(|b| !matches!(&b.who, Subject::Principal(n) if n == name));
    }

    /// What each principal holds.
    fn index(&self) -> HashMap<String, Arc<[Held]>> {
        let mut held: HashMap<String, Vec<Held>> = HashMap::new();
        for b in &self.bindings {
            let Subject::Principal(name) = &b.who;
            held.entry(name.clone()).or_default().push(Held { role: b.role, scope: b.scope });
        }
        held.into_iter().map(|(name, h)| (name, Arc::from(h))).collect()
    }
}

/// A version of the document and its index: made once per version, so `can` reads what each
/// principal holds and never the document (§7's performance note).
struct Version {
    document: Arc<Document>,
    held: HashMap<String, Arc<[Held]>>,
}

/// The access document as the server holds it: its newest version, kept where it was read from.
pub(crate) struct Bindings {
    /// None in memory (the tests).
    path: Option<PathBuf>,
    now: RwLock<Version>,
    /// Whether the history may end in a line cut short (the server stopped, or a write failed,
    /// while one was written): the next is put on a line of its own.
    torn: AtomicBool,
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
        Bindings { path, now: RwLock::new(Version { document: Arc::new(document), held }), torn: AtomicBool::new(false) }
    }

    /// The newest version.
    pub(crate) fn document(&self) -> Arc<Document> {
        Arc::clone(&self.now.read().unwrap().document)
    }

    /// What `name` holds now: nothing, if nothing is bound to it.
    pub(crate) fn held(&self, name: &str) -> Arc<[Held]> {
        self.now.read().unwrap().held.get(name).cloned().unwrap_or_else(|| Arc::from([]))
    }

    /// Keep `document` as the newest version. It is kept once it is in the history, and in force
    /// from then on: written as the document after, so the document is never ahead of its
    /// history, and read from the history at the next start if that write fails. The accounts
    /// book's lock is held (accounts.rs), so versions are kept one at a time.
    pub(crate) fn keep(&self, document: Document) -> Result<(), String> {
        if let Some(path) = &self.path {
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
        let held = document.index();
        *self.now.write().unwrap() = Version { document: Arc::new(document), held };
        Ok(())
    }

    /// Version `n`, from the history.
    #[cfg(test)]
    pub(crate) fn version(&self, n: u64) -> Option<Document> {
        let text = std::fs::read_to_string(history(self.path.as_ref()?)).ok()?;
        text.lines().filter_map(|l| serde_json::from_str::<Document>(l).ok()).find(|d| d.version == n)
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
        d.after(document, by, what, 1_800_000_000 + document.version)
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
        assert_eq!(&*access.held("bob"), [Held { role: Role::Viewer, scope: Scope::Host }], "in place of editor");
        assert_eq!(three.bindings.iter().map(|b| b.id).collect::<Vec<_>>(), [0, 2], "ids aren't reused");
        let mut same = three.clone();
        same.set_host_role("bob", Role::Viewer, "ada", 4);
        assert_eq!(same, three, "the role bound there already: nothing changes");

        let again = Bindings::open(path).unwrap();
        assert_eq!(*again.document(), three);
        assert_eq!([1, 2, 3].map(|n| again.version(n)), [Some(one), Some(two), Some(three)]);
        assert_eq!(&*again.held("ada"), [Held { role: Role::Admin, scope: Scope::Host }]);
        assert!(again.held("cy").is_empty());
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
        assert_eq!(access.version(3), Some(three));
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
