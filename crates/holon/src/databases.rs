//! Databases (`docs/L4s/databases-L4.md`): SQL databases a flow reads and writes on the host.
//!
//! - **Bound by an admin,** as a folder is (`--databases`, owner-only; `/admin/databases`,
//!   `holon database`): a name a flow uses (`database shop`) bound to a database (SQLite: a file on
//!   this machine; Postgres or MySQL: a connection, named as a secret's source is, `env:…` or
//!   `vault:…`, and fetched by the host: engines.rs), with the most it allows (read, or read and
//!   write), for the whole server or one deployment. A flow never sees where it is. The admin's
//!   schema (`--schema`) is run when it is bound; a flow runs no DDL.
//! - **Reads** (`shop.query`, `shop.one`) are answered on the spot, from the database as the run
//!   found it, only from the tables the deployment was granted (the host reads the statement's
//!   tables again: sql.rs), within its [`DbLimits`]; each is kept with the run's reads, so the log
//!   shows it, replay and `holon verify` answer from it, and the run is checked against it when it
//!   commits. A read past a limit fails the run: a list cut short would be wrong without knowing.
//! - **Writes** (`shop.exec`) are effects, sent when the run finishes, in one transaction per
//!   database ([`commit`]): it runs the run's reads again and goes round again if one changed,
//!   applies each statement checking its `expect`, and marks the run done in the database's own
//!   `holon_commits` table, in the same transaction, so a run applied twice (a resume after a stop)
//!   applies nothing the second time. The transaction stays open while the run's other writes land,
//!   then commits: what it checked holds until then.

use std::sync::{MutexGuard, RwLock};
use std::time::Instant;

use rouille::{Request, Response};
use rusqlite::types::{Value as Sql, ValueRef};
use rusqlite::{Connection, OpenFlags};

use crate::claims::ClaimKey;
use crate::engines::{Failure, Remote};
use crate::folders::FolderAccess;
use crate::http::{err, read_body};
use crate::permissions::{DbLimits, Manifest, Permit};
use crate::secret_sources::{Reference, Sources};

use crate::*;

/// The table in each writable database where a run's commit marks it done (its id).
pub(crate) const COMMITS_TABLE: &str = "holon_commits";
/// What the error of a run whose database read failed starts with: it answers 500.
pub(crate) const READ_FAILED: &str = "a database read failed: ";
/// What the error of a run whose database writes didn't land starts with: it answers 500.
pub(crate) const WRITE_FAILED: &str = "a database write didn't land: ";
/// What the error of a run whose statement changed other than the rows it expects starts with:
/// nothing it did was written, and it answers 409 (the data said no; it isn't run again).
pub(crate) const UNMET: &str = "a statement didn't change the rows it expects: ";

/// One bound database.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Database {
    /// Which engine: `sqlite`, `postgres` or `mysql`.
    pub(crate) engine: String,
    /// A SQLite database's file, as bound (absolute, links resolved).
    #[serde(default, skip_serializing_if = "is_empty_path")]
    pub(crate) path: PathBuf,
    /// A Postgres or MySQL database's connection: where the host fetches its URL from (`env:…`,
    /// `vault:…`), never the URL itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) connection: Option<String>,
    /// Where that connection points, without its credentials (`postgres://host:5432/shop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) address: Option<String>,
    pub(crate) access: FolderAccess,
    /// Its runs hold it for their whole length (`run`), not only across their commits.
    #[serde(default, skip_serializing_if = "locks::Setting::is_commit")]
    pub(crate) lock: locks::Setting,
}

fn is_empty_path(p: &Path) -> bool {
    p.as_os_str().is_empty()
}

impl Database {
    /// What the database is known by: its file, or where its connection points. Two bindings of
    /// one database are one database to claims and commit locks.
    pub(crate) fn at(&self) -> String {
        self.address.clone().unwrap_or_else(|| self.path.display().to_string())
    }
}

/// An open database: SQLite's one connection, or a pool of them to a Postgres or MySQL.
#[derive(Clone)]
enum Open {
    Sqlite(Arc<Mutex<Connection>>),
    Remote(Arc<Remote>),
}

/// Every binding, as [`folders::Bound`]: the server's by name, each deployment's under
/// `@deployments`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Bound {
    #[serde(rename = "@deployments", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deployments: BTreeMap<String, BTreeMap<String, Database>>,
    #[serde(flatten)]
    pub(crate) server: BTreeMap<String, Database>,
}

/// The databases bound on this server, where they are kept, and one connection to each.
pub(crate) struct Databases {
    file: Option<PathBuf>,
    /// The server's own files and folders: never bound, nor anything inside them.
    forbidden: Vec<PathBuf>,
    bound: RwLock<Bound>,
    /// Each database open, every deployment's runs through it, by what it is known by
    /// ([`Database::at`]).
    open: Mutex<BTreeMap<String, Open>>,
    /// What a connection's source is fetched from: the server's environment and dotenv file.
    sources: Sources,
}

impl std::fmt::Debug for Databases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Databases").field("bound", &self.bound.read().unwrap()).finish()
    }
}

impl Databases {
    /// None, kept nowhere: the tests, and a server told `--databases :memory:`.
    pub(crate) fn memory() -> Databases {
        Databases { file: None, forbidden: Vec::new(), bound: RwLock::new(Bound::default()), open: Mutex::new(BTreeMap::new()), sources: Sources::none() }
    }

    /// These databases, fetching connections from `sources`.
    pub(crate) fn with_sources(mut self, sources: Sources) -> Databases {
        self.sources = sources;
        self
    }

    /// The databases bound in `file` (none yet if it doesn't exist), refusing ever to bind one in
    /// `forbidden`.
    pub(crate) fn open(file: &str, forbidden: &[PathBuf]) -> Result<Databases, String> {
        let forbidden: Vec<PathBuf> = forbidden.iter().map(|p| real_file(p)).collect();
        if file == ":memory:" {
            return Ok(Databases { forbidden, ..Databases::memory() });
        }
        let path = PathBuf::from(file);
        let bound = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{file} isn't a databases file: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bound::default(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        Ok(Databases { file: Some(path), forbidden, bound: RwLock::new(bound), ..Databases::memory() })
    }

    /// Every database bound: the server's, and each deployment's.
    pub(crate) fn all(&self) -> Bound {
        self.bound.read().unwrap().clone()
    }

    /// The database deployment `deployment` sees as `name`: its own binding, else the server's.
    pub(crate) fn resolve(&self, deployment: &str, name: &str) -> Option<Database> {
        let bound = self.bound.read().unwrap();
        bound.deployments.get(deployment).and_then(|d| d.get(name)).or_else(|| bound.server.get(name)).cloned()
    }

    /// Bind `name` to a database, for the whole server or one deployment, replacing any binding of
    /// it there before: `target` is a SQLite database's file, or where a Postgres or MySQL
    /// database's connection URL is fetched from (`env:HOLON_SECRET_…`, `vault:…`). With a `schema`
    /// (the admin's DDL), it is run first (and a SQLite file created, if there is none); without
    /// one, a SQLite file must be a database already.
    #[cfg(test)]
    pub(crate) fn bind(&self, name: &str, target: &str, access: FolderAccess, schema: Option<&str>, for_deployment: Option<&str>) -> Result<Database, String> {
        self.bind_as(name, target, access, schema, locks::Setting::Commit, for_deployment)
    }

    /// As [`bind`](Self::bind), saying how long its runs hold it (`lock`).
    pub(crate) fn bind_as(&self, name: &str, target: &str, access: FolderAccess, schema: Option<&str>, lock: locks::Setting, for_deployment: Option<&str>) -> Result<Database, String> {
        if !folders::is_folder_name(name) {
            return Err(format!("{name:?} isn't a database name: letters, digits, - and _, up to 64"));
        }
        if let Some(slug) = for_deployment.filter(|s| !folders::is_folder_name(s)) {
            return Err(format!("{slug:?} isn't a deployment's name"));
        }
        let schema = schema.map(str::trim).filter(|s| !s.is_empty());
        let target = target.trim();
        let (mut database, open) = if target.starts_with("env:") || target.starts_with("vault:") {
            self.open_remote(target, access, schema, lock)?
        } else {
            self.open_sqlite(target, access, schema)?
        };
        database.lock = lock;
        let mut bound = self.bound.write().unwrap();
        let mut next = bound.clone();
        match for_deployment {
            Some(slug) => next.deployments.entry(slug.to_string()).or_default().insert(name.to_string(), database.clone()),
            None => next.server.insert(name.to_string(), database.clone()),
        };
        self.save(&next)?;
        *bound = next;
        self.open.lock().unwrap().insert(database.at(), open);
        Ok(database)
    }

    /// The Postgres or MySQL database whose URL `source` holds, reached now, its schema run.
    fn open_remote(&self, source: &str, access: FolderAccess, schema: Option<&str>, lock: locks::Setting) -> Result<(Database, Open), String> {
        let reference = Reference::parse(source)?;
        let url = self.sources.fetch(&reference)?;
        let remote = Remote::open(url)?;
        if let Some(schema) = schema {
            remote.run_schema(schema)?;
        }
        // A run lock is held in the database too, on MySQL in holon_claims, read only or not.
        if access == FolderAccess::ReadWrite || lock == locks::Setting::Run {
            remote.keep_tables()?;
        }
        let database = Database {
            engine: remote.engine.name().into(),
            path: PathBuf::new(),
            connection: Some(source.to_string()),
            address: Some(remote.address.clone()),
            access,
            lock,
        };
        Ok((database, Open::Remote(Arc::new(remote))))
    }

    /// The SQLite database at `path`, its schema run (the file made if there is none).
    fn open_sqlite(&self, path: &str, access: FolderAccess, schema: Option<&str>) -> Result<(Database, Open), String> {
        let given = PathBuf::from(path);
        if !given.is_absolute() {
            return Err(format!("{path:?} isn't an absolute path: give the whole path, as this server's machine sees it (or a connection: env:HOLON_SECRET_… or vault:…)"));
        }
        if !given.is_file() && schema.is_none() {
            return Err(format!("there is no database at {path:?}: give a schema to create one there"));
        }
        if given.parent().is_none_or(|p| !p.is_dir()) {
            return Err(format!("{path:?} isn't in a folder on this server's machine"));
        }
        let at = real_file(&given);
        if let Some(own) = self.forbidden.iter().find(|f| at.starts_with(f) || *f == &at) {
            return Err(format!("{} can't be bound: it is, or is inside, this server's own data ({})", at.display(), own.display()));
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | if schema.is_some() { OpenFlags::SQLITE_OPEN_CREATE } else { OpenFlags::empty() };
        let conn = Connection::open_with_flags(&at, flags).map_err(|e| format!("{} can't be opened as a SQLite database: {e}", at.display()))?;
        // Whether it is one at all: a file that isn't says so here, not at a run.
        conn.query_row("select count(*) from sqlite_master", [], |_| Ok(())).map_err(|e| format!("{} isn't a SQLite database: {e}", at.display()))?;
        if let Some(schema) = schema {
            conn.execute_batch(&format!("begin; {schema}; commit;")).map_err(|e| {
                let _ = conn.execute_batch("rollback");
                format!("the schema didn't run: {e}")
            })?;
        }
        if access == FolderAccess::ReadWrite {
            conn.execute_batch(&format!("create table if not exists {COMMITS_TABLE} (run text primary key, at integer not null)"))
                .map_err(|e| format!("can't keep {COMMITS_TABLE} in it: {e}"))?;
        }
        let database = Database { engine: "sqlite".into(), path: at, connection: None, address: None, access, lock: locks::Setting::Commit };
        Ok((database, Open::Sqlite(Arc::new(Mutex::new(conn)))))
    }

    /// Unbind `name`, the server's or one deployment's: whether it was bound. The database stays.
    pub(crate) fn unbind(&self, name: &str, for_deployment: Option<&str>) -> Result<bool, String> {
        let mut bound = self.bound.write().unwrap();
        let mut next = bound.clone();
        let removed = match for_deployment {
            Some(slug) => {
                let removed = next.deployments.get_mut(slug).and_then(|d| d.remove(name)).is_some();
                if next.deployments.get(slug).is_some_and(BTreeMap::is_empty) {
                    next.deployments.remove(slug);
                }
                removed
            }
            None => next.server.remove(name).is_some(),
        };
        if !removed {
            return Ok(false);
        }
        self.save(&next)?;
        *bound = next;
        Ok(true)
    }

    fn save(&self, bound: &Bound) -> Result<(), String> {
        let Some(file) = &self.file else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(bound).map_err(|e| e.to_string())?;
        crate::sealing::write_private(file, &bytes).map_err(|e| format!("can't write {}: {e}", file.display()))
    }

    /// `database`, opened on first use: a SQLite file read and write, or read only where it can't
    /// be written (read-only media); a Postgres or MySQL database by fetching its connection again.
    fn connection(&self, database: &Database) -> Result<Open, String> {
        let at = database.at();
        if let Some(c) = self.open.lock().unwrap().get(&at) {
            return Ok(c.clone());
        }
        let opened = match &database.connection {
            Some(source) => {
                let url = self.sources.fetch(&Reference::parse(source)?)?;
                Open::Remote(Arc::new(Remote::open(url)?))
            }
            None => {
                let rw = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
                let conn = Connection::open_with_flags(&database.path, rw)
                    .or_else(|_| Connection::open_with_flags(&database.path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX))
                    .map_err(|e| format!("the database at {} can't be opened: {e}", database.path.display()))?;
                Open::Sqlite(Arc::new(Mutex::new(conn)))
            }
        };
        // Two runs may have opened it at once: the first one in is the one kept.
        Ok(self.open.lock().unwrap().entry(at).or_insert(opened).clone())
    }

    /// The Postgres or MySQL database open at `at`, if one is.
    fn remote_at(&self, at: &str) -> Option<Arc<Remote>> {
        match self.open.lock().unwrap().get(at) {
            Some(Open::Remote(r)) => Some(Arc::clone(r)),
            _ => None,
        }
    }

    /// Hold `keys` for `holder` in each Postgres or MySQL database they are in, all or none,
    /// without waiting (engines.rs): the first another server's run holds, otherwise. Keys of a
    /// SQLite database, a folder or a collection hold nothing here.
    pub(crate) fn claim_in_databases(&self, keys: &[ClaimKey], holder: &str) -> Result<(), ClaimKey> {
        let mut by_database: BTreeMap<String, Vec<ClaimKey>> = BTreeMap::new();
        for k in keys {
            if let ClaimKey::Row { at, .. } | ClaimKey::Table { at, .. } | ClaimKey::NewRow { at, .. } = k {
                by_database.entry(at.clone()).or_default().push(k.clone());
            }
        }
        let mut taken: Vec<(Arc<Remote>, Vec<ClaimKey>)> = Vec::new();
        for (at, keys) in by_database {
            let Some(remote) = self.remote_at(&at) else { continue };
            if let Err(key) = remote.claim(&keys, holder) {
                for (r, k) in &taken {
                    r.unclaim(k, holder);
                }
                return Err(key);
            }
            taken.push((remote, keys));
        }
        Ok(())
    }

    /// Let go of `keys` held for `holder` in the databases they are in.
    pub(crate) fn unclaim_in_databases(&self, keys: &[ClaimKey], holder: &str) {
        let mut by_database: BTreeMap<String, Vec<ClaimKey>> = BTreeMap::new();
        for k in keys {
            if let ClaimKey::Row { at, .. } | ClaimKey::Table { at, .. } | ClaimKey::NewRow { at, .. } = k {
                by_database.entry(at.clone()).or_default().push(k.clone());
            }
        }
        for (at, keys) in by_database {
            if let Some(remote) = self.remote_at(&at) {
                remote.unclaim(&keys, holder);
            }
        }
    }

    /// Why a deployment asking for `manifest` can't run here, if it can't: a database it needs
    /// that isn't bound, or is bound read only when it writes.
    pub(crate) fn refuses(&self, deployment: &str, manifest: &Manifest) -> Option<String> {
        for item in &manifest.capabilities {
            if manifest.requirements.optional.contains(item) {
                continue;
            }
            let (table, writes) = match (item.strip_prefix("DbRead(\""), item.strip_prefix("DbWrite(\"")) {
                (Some(rest), _) => (rest.strip_suffix("\")")?, false),
                (_, Some(rest)) => (rest.strip_suffix("\")")?, true),
                _ => continue,
            };
            let name = table.split('/').next().unwrap_or(table);
            match self.resolve(deployment, name) {
                None => return Some(unbound(name, deployment)),
                Some(d) if writes && d.access != FolderAccess::ReadWrite => {
                    return Some(format!("the database {name:?} is bound read-only on this server, and this flow writes it"));
                }
                Some(_) => {}
            }
        }
        None
    }

    /// The commit locks a run's commit takes on its databases (locks.rs): each it read or writes,
    /// held alone, by its file or address, so two names for one database are one lock. Its own
    /// transaction is the lock against other programs and servers.
    pub(crate) fn commit_locks(&self, deployment: &str, reads: &[collections::Read], performed: &[(Capability, Value)]) -> Vec<locks::Wanted> {
        self.locks_on(deployment, names_in(reads, performed))
    }

    /// The run locks (§7.3) a run of `deployment` takes before it starts: on each database it may
    /// read (shared) or write (exclusive) that an admin set to `run`, by its file or address, in the
    /// server's table; and the Postgres and MySQL ones among them, to hold in the database too,
    /// with whether exclusive and how it is said, for the runs of other servers.
    #[allow(clippy::type_complexity)]
    pub(crate) fn whole_run_locks(&self, deployment: &str, read: &BTreeSet<String>, written: &BTreeSet<String>) -> (Vec<locks::Wanted>, Vec<(Arc<Remote>, bool, String)>) {
        let (mut wanted, mut remote) = (Vec::new(), Vec::new());
        for name in read.union(written) {
            let Some(d) = self.resolve(deployment, name).filter(|d| d.lock == locks::Setting::Run) else { continue };
            let exclusive = written.contains(name);
            let mode = if exclusive { locks::Mode::Exclusive } else { locks::Mode::Shared };
            let label = format!("the database {name:?}");
            wanted.push(locks::Wanted { address: format!("run:db:{}", d.at()), label: label.clone(), mode, file: None });
            // One that can't be reached holds nothing there: its run's first read says why.
            if let Ok(Open::Remote(r)) = self.connection(&d) {
                remote.push((r, exclusive, label));
            }
        }
        // In the order of their addresses, as the server's locks are taken.
        remote.sort_by(|a, b| a.0.address.cmp(&b.0.address));
        (wanted, remote)
    }

    /// The locks on the databases `names` (as `deployment` knows them), each held alone: what a
    /// commit takes, and what the last try holds for its whole run, so no other deployment's
    /// commit changes them under it.
    pub(crate) fn locks_on(&self, deployment: &str, names: BTreeSet<String>) -> Vec<locks::Wanted> {
        names
            .into_iter()
            .filter_map(|name| {
                let d = self.resolve(deployment, &name)?;
                Some(locks::Wanted { address: format!("db:{}", d.at()), label: format!("the database {name:?}"), mode: locks::Mode::Exclusive, file: None })
            })
            .collect()
    }
}

/// The path with its links resolved where it exists; a file not made yet, its folder's.
fn real_file(path: &Path) -> PathBuf {
    if let Ok(p) = folders::canonical(path) {
        return p;
    }
    match (path.parent().and_then(|p| folders::canonical(p).ok()), path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

fn unbound(name: &str, deployment: &str) -> String {
    format!("the database {name:?} isn't bound on this server: an admin binds it (holon database add {name} PATH|env:…, or --for {deployment} for this deployment only)")
}

/// The databases a run read or writes, by the name its deployment knows them by.
fn names_in(reads: &[collections::Read], performed: &[(Capability, Value)]) -> BTreeSet<String> {
    let read = reads.iter().filter_map(|r| match &r.request {
        Value::Record(q) => match q.get("database") {
            Some(Value::Text(n)) => Some(n.clone()),
            _ => None,
        },
        _ => None,
    });
    let written = performed.iter().filter_map(|(cap, _)| match cap {
        Capability::DbWrite(t) => Some(t.split('/').next().unwrap_or(t).to_string()),
        _ => None,
    });
    read.chain(written).collect()
}

// ── claims (docs/L4s/databases-L4.md §7.2) ──

/// The claims a database read takes on each table it reads that this deployment may write
/// (`written`, as `shop/orders`): the row its `where` pins by the table's whole primary key, or
/// else the whole table.
pub(crate) fn claims_of_read(folders: &folders::Scoped, request: &Value, written: &BTreeSet<String>) -> Vec<ClaimKey> {
    let Value::Record(q) = request else { return Vec::new() };
    let (Some(Value::Text(name)), Some(Value::Text(sql)), Some(Value::List(params))) = (q.get("database"), q.get("query"), q.get("params")) else {
        return Vec::new();
    };
    claims_on(folders, name, sql, params, false).into_iter().filter(|k| written.contains(&format!("{name}/{}", table_of_key(k)))).collect()
}

/// The claims the statements a run sends take: the row each one's `where` pins (for an insert,
/// the row its values name), or the whole table.
pub(crate) fn claims_of_writes(folders: &folders::Scoped, performed: &[(Capability, Value)]) -> Vec<ClaimKey> {
    let mut keys = Vec::new();
    for (cap, payload) in performed {
        let (Capability::DbWrite(t), Value::Record(e)) = (cap, payload) else { continue };
        let (Some(Value::Text(sql)), Some(Value::List(params))) = (e.get("sql"), e.get("params")) else { continue };
        keys.extend(claims_on(folders, t.split('/').next().unwrap_or(t), sql, params, true));
    }
    keys
}

/// Every table a deployment may read or write, whole.
pub(crate) fn tables_of(folders: &folders::Scoped, permit: &Permit) -> Vec<ClaimKey> {
    let tables: BTreeSet<String> = permit.tables_read().into_iter().chain(permit.tables_written()).collect();
    tables
        .iter()
        .filter_map(|t| {
            let (name, table) = t.split_once('/')?;
            let d = folders.all.databases.resolve(&folders.deployment, name)?;
            Some(ClaimKey::Table { at: d.at(), name: name.to_string(), table: table.to_string() })
        })
        .collect()
}

fn table_of_key(k: &ClaimKey) -> &str {
    match k {
        ClaimKey::Row { table, .. } | ClaimKey::Table { table, .. } | ClaimKey::NewRow { table, .. } => table,
        _ => "",
    }
}

/// The keys a statement on the database `name` takes: on each table it reads (`write`: writes),
/// its row when the statement pins the table's primary key, else the table; an insert that
/// doesn't name its row, a new row of the table.
fn claims_on(folders: &folders::Scoped, name: &str, sql: &str, params: &[Value], write: bool) -> Vec<ClaimKey> {
    let Some(d) = folders.all.databases.resolve(&folders.deployment, name) else { return Vec::new() };
    let Ok(tables) = holon_core::sql::tables(sql, write) else { return Vec::new() };
    let at = d.at();
    let insert = write && sql.trim_start().get(..6).is_some_and(|w| w.eq_ignore_ascii_case("insert") || w.eq_ignore_ascii_case("replac"));
    let row = holon_core::sql::pins(sql, write).and_then(|(table, pinned)| {
        let key = primary_key(folders, &d, &table)?;
        let values: Option<Vec<serde_json::Value>> = key.iter().map(|c| pinned.get(c).and_then(|n| params.get(n - 1)).map(claim_value)).collect();
        Some((table, serde_json::Value::Array(values?).to_string()))
    });
    let touched = if write { &tables.writes } else { &tables.reads };
    touched
        .iter()
        .map(|table| {
            let (at, name, table) = (at.clone(), name.to_string(), table.clone());
            match &row {
                Some((t, key)) if *t == table => ClaimKey::Row { at, name, table, key: key.clone() },
                _ if insert => ClaimKey::NewRow { at, name, table },
                _ => ClaimKey::Table { at, name, table },
            }
        })
        .collect()
}

/// The columns of `table`'s primary key, in order and in lower case; none for a table without
/// one, or one that isn't there.
fn primary_key(folders: &folders::Scoped, d: &Database, table: &str) -> Option<Vec<String>> {
    let conn = match folders.all.databases.connection(d).ok()? {
        Open::Sqlite(c) => c,
        Open::Remote(r) => return r.primary_key(table),
    };
    let conn = conn.lock().unwrap();
    let (schema, table) = table.split_once('.').unwrap_or(("main", table));
    let mut stmt = conn.prepare("select lower(name) from pragma_table_info(?1, ?2) where pk > 0 order by pk").ok()?;
    let columns: Vec<String> = stmt.query_map(rusqlite::params![table, schema], |r| r.get(0)).ok()?.collect::<Result<_, _>>().ok()?;
    (!columns.is_empty()).then_some(columns)
}

/// A key's value as a claim compares it: a whole number, from a number or a text that is one;
/// any other text in lower case. Two values a column could hold as one row are one key.
fn claim_value(v: &Value) -> serde_json::Value {
    match v {
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => serde_json::json!(*n as i64),
        Value::Text(t) if t.trim().parse::<i64>().is_ok() => serde_json::json!(t.trim().parse::<i64>().unwrap()),
        Value::Text(t) => serde_json::json!(t.to_lowercase()),
        Value::Boolean(b) => serde_json::json!(i64::from(*b)),
        other => serde_json::json!(holon_core::stringify_json(other).unwrap_or_default().to_lowercase()),
    }
}

// ── values ──

/// A statement's parameter, from a Holon value: a whole number as an integer, true and false as
/// 1 and 0, none as null, a record or list as its JSON text.
fn param(v: &Value) -> Result<Sql, String> {
    Ok(match v {
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => Sql::Integer(*n as i64),
        Value::Number(n) if n.is_finite() => Sql::Real(*n),
        Value::Number(_) => return Err("a parameter is a finite number".into()),
        Value::Text(t) => Sql::Text(t.clone()),
        Value::Boolean(b) => Sql::Integer(i64::from(*b)),
        Value::Sum { arm: 1, .. } => Sql::Null,
        Value::Sum { payload, .. } => param(payload)?,
        Value::List(_) | Value::Record(_) => Sql::Text(holon_core::stringify_json(v).ok_or("a parameter's record or list has a value JSON can't hold")?),
        Value::Effect { .. } => return Err("an effect is never a parameter".into()),
    })
}

/// A column's value as a Holon value: a number, a text, bytes as hex; null as nothing (the
/// record has no such field). An integer too big for a number exactly is its digits, as text.
fn column(v: ValueRef<'_>) -> Option<Value> {
    Some(match v {
        ValueRef::Null => return None,
        ValueRef::Integer(n) if n.unsigned_abs() < 1 << 53 => Value::Number(n as f64),
        ValueRef::Integer(n) => Value::Text(n.to_string()),
        ValueRef::Real(f) => Value::Number(f),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::Text(b.iter().map(|x| format!("{x:02x}")).collect()),
    })
}

/// The bytes a value counts for against `max_db_read_bytes`.
fn size_of(v: &Value) -> u64 {
    match v {
        Value::Text(t) => t.len() as u64,
        _ => 8,
    }
}

/// What a query asked: `{database, query, params, one}`.
struct Asked<'r> {
    database: &'r str,
    sql: &'r str,
    params: &'r [Value],
    one: bool,
}

fn asked(request: &Value) -> Result<Asked<'_>, String> {
    let Value::Record(r) = request else { return Err("a database read is a record".into()) };
    let (Some(Value::Text(database)), Some(Value::Text(sql)), Some(Value::List(params))) = (r.get("database"), r.get("query"), r.get("params")) else {
        return Err("a database read names its database, query and params".into());
    };
    Ok(Asked { database, sql, params, one: matches!(r.get("one"), Some(Value::Boolean(true))) })
}

/// Why a statement failed, said for the run log.
fn said(e: &rusqlite::Error, sql: &str) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => {
            format!("{sql:?} ran past db_statement_ms")
        }
        e => format!("{sql:?}: {e}"),
    }
}

fn busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

/// Run `f` on `conn`, interrupted past `until`.
fn timed<T>(conn: &Connection, until: Instant, f: impl FnOnce(&Connection) -> T) -> T {
    conn.progress_handler(1000, Some(move || Instant::now() > until));
    let out = f(conn);
    conn.progress_handler(0, None::<fn() -> bool>);
    out
}

/// The rows a read gathers, as records by column name: at most one if `one`, failing past
/// `rows` rows or `bytes` bytes, on every engine alike.
pub(crate) struct Gather<'s> {
    sql: &'s str,
    one: bool,
    rows: u64,
    bytes: u64,
    pub(crate) out: Vec<Value>,
    pub(crate) size: u64,
}

impl<'s> Gather<'s> {
    pub(crate) fn new(sql: &'s str, one: bool, rows: u64, bytes: u64) -> Gather<'s> {
        Gather { sql, one, rows, bytes, out: Vec::new(), size: 0 }
    }

    /// Take one row: whether to go on to the next. `Err` past a limit.
    pub(crate) fn take(&mut self, record: BTreeMap<String, Value>) -> Result<bool, String> {
        if self.out.len() as u64 >= self.rows {
            return Err(format!("{:?} gives more than max_db_rows ({}) rows", self.sql, self.rows));
        }
        self.size += record.iter().map(|(name, v)| size_of(v) + name.len() as u64).sum::<u64>();
        if self.size > self.bytes {
            return Err(format!("{:?} gives more than max_db_read_bytes ({})", self.sql, self.bytes));
        }
        self.out.push(Value::Record(record));
        Ok(!self.one)
    }
}

/// The rows `sql` selects from a SQLite database, gathered into `g`. Only a statement the
/// engine says writes nothing is run.
fn select(conn: &Connection, sql: &str, params: &[Value], until: Instant, g: &mut Gather) -> Result<(), String> {
    let params: Vec<Sql> = params.iter().map(param).collect::<Result<_, _>>()?;
    timed(conn, until, |conn| {
        let mut stmt = conn.prepare(sql).map_err(|e| said(&e, sql))?;
        if !stmt.readonly() {
            return Err(format!("{sql:?} writes: a query reads"));
        }
        let names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
        let mut found = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| said(&e, sql))?;
        while let Some(row) = found.next().map_err(|e| said(&e, sql))? {
            let mut record = BTreeMap::new();
            for (i, name) in names.iter().enumerate() {
                if let Some(v) = column(row.get_ref(i).map_err(|e| said(&e, sql))?) {
                    record.insert(name.clone(), v);
                }
            }
            if !g.take(record)? {
                break;
            }
        }
        Ok(())
    })
}

/// What a query answers: its rows, or for `one` the first, or nothing when there is none.
fn answer_of(asked: &Asked, rows: Vec<Value>) -> Option<Value> {
    if asked.one { rows.into_iter().next() } else { Some(Value::List(rows)) }
}

// ── reads ──

/// The databases one run may read, and how much it has read.
pub(crate) struct Readable {
    pub(crate) folders: Arc<folders::Scoped>,
    /// The tables it was granted, as `shop/orders`.
    granted: BTreeSet<String>,
    limits: DbLimits,
    rows: u64,
    bytes: u64,
    statements: u64,
}

impl std::fmt::Debug for Readable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readable").field("granted", &self.granted).field("rows", &self.rows).field("statements", &self.statements).finish()
    }
}

impl Readable {
    pub(crate) fn new(folders: Arc<folders::Scoped>, permit: &Permit) -> Readable {
        Readable { folders, granted: permit.tables_read(), limits: permit.databases.clone(), rows: 0, bytes: 0, statements: 0 }
    }

    /// The answer to a query (`{database, query, params, one}`), from the database as it is now,
    /// within what the run may still read. `Err`: why it fails the run.
    pub(crate) fn answer(&mut self, request: &Value) -> Result<Option<Value>, String> {
        let asked = asked(request)?;
        let all = &self.folders.all.databases;
        let database = all.resolve(&self.folders.deployment, asked.database).ok_or_else(|| unbound(asked.database, &self.folders.deployment))?;
        let tables = holon_core::sql::tables(asked.sql, false)?;
        if let Some(t) = tables.reads.iter().find(|t| !self.granted.contains(&format!("{}/{t}", asked.database))) {
            return Err(format!("the table {t:?} of the database {:?} isn't granted to this deployment", asked.database));
        }
        self.statements += 1;
        if self.statements > self.limits.max_db_statements {
            return Err(format!("the run sends more than max_db_statements ({})", self.limits.max_db_statements));
        }
        let rows_left = self.limits.max_db_rows.saturating_sub(self.rows);
        let bytes_left = self.limits.max_db_read_bytes.saturating_sub(self.bytes);
        let mut g = Gather::new(asked.sql, asked.one, rows_left, bytes_left);
        let read = match all.connection(&database)? {
            Open::Sqlite(conn) => select(&conn.lock().unwrap(), asked.sql, asked.params, Instant::now() + self.limits.db_statement, &mut g),
            Open::Remote(remote) => remote.read(asked.sql, asked.params, &mut g, self.limits.db_statement),
        };
        read.map_err(|e| e.replace("max_db_rows (", "max_db_rows (left: ").replace("max_db_read_bytes (", "max_db_read_bytes (left: "))?;
        self.rows += g.out.len() as u64;
        self.bytes += g.size;
        Ok(answer_of(&asked, g.out))
    }
}

// ── commit ──

/// How a run's database commit went.
pub(crate) enum Commit<T> {
    /// Committed, with what its other writes gave (`then`), and any database that failed to
    /// commit at the very end.
    Done(T, Vec<String>),
    /// A row it read changed, or a database was held past `commit_wait`: nothing was written;
    /// it runs again.
    Again(String),
    /// It can't commit: nothing was written, and why (starting [`UNMET`] for an `expect` the data
    /// didn't meet, else [`WRITE_FAILED`]).
    Refused(String),
}

/// One database's open transaction in a commit.
enum Held<'a> {
    Sqlite(MutexGuard<'a, Connection>),
    Remote(crate::engines::Txn<'a>),
}

impl Held<'_> {
    /// Roll it back (a remote one rolls back as it is dropped).
    fn undo(self) {
        if let Held::Sqlite(c) = self {
            let _ = c.execute_batch("rollback");
        }
    }
}

/// Commit a run's database writes, each database in one transaction, in the order of what they
/// are known by: run its reads again (with `check`) and go round again if one changed; apply each
/// statement, checking its `expect`; mark the run done in `holon_commits` (a run marked there
/// already applies nothing again). A Postgres or MySQL transaction then commits; with every
/// SQLite one still open, `then` lands the run's other writes, and those commit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit<T>(
    folders: Option<&folders::Scoped>,
    run_id: &str,
    reads: &[collections::Read],
    performed: &[(Capability, Value)],
    permit: &Permit,
    limits: &holon_host::Limits,
    check: bool,
    then: impl FnOnce() -> T,
) -> Commit<T> {
    let Some(folders) = folders else { return Commit::Done(then(), Vec::new()) };
    let all = &folders.all.databases;
    // Each database's reads and writes, by what it is known by.
    struct Work<'r> {
        name: String,
        database: Database,
        reads: Vec<&'r collections::Read>,
        execs: Vec<&'r Value>,
    }
    let mut work: BTreeMap<String, Work> = BTreeMap::new();
    let mut place = |name: &str| -> Result<String, String> {
        let database = all.resolve(&folders.deployment, name).ok_or_else(|| unbound(name, &folders.deployment))?;
        let at = database.at();
        work.entry(at.clone()).or_insert(Work { name: name.to_string(), database, reads: Vec::new(), execs: Vec::new() });
        Ok(at)
    };
    let mut placed_reads = Vec::new();
    for r in reads {
        if let Value::Record(q) = &r.request
            && let Some(Value::Text(name)) = q.get("database")
        {
            match place(name) {
                Ok(p) => placed_reads.push((p, r)),
                Err(e) => return Commit::Refused(format!("{WRITE_FAILED}{e}")),
            }
        }
    }
    let mut placed_execs = Vec::new();
    for (cap, payload) in performed {
        if let Capability::DbWrite(t) = cap {
            match place(t.split('/').next().unwrap_or(t)) {
                Ok(p) => placed_execs.push((p, payload)),
                Err(e) => return Commit::Refused(format!("{WRITE_FAILED}{e}")),
            }
        }
    }
    // Its reads counted as it sent them (`Readable`); its writes count too.
    let max = permit.databases.max_db_statements;
    if (placed_reads.len() + placed_execs.len()) as u64 > max {
        return Commit::Refused(format!("{WRITE_FAILED}the run sends more than max_db_statements ({max})"));
    }
    for (p, r) in placed_reads {
        work.get_mut(&p).expect("placed").reads.push(r);
    }
    for (p, e) in placed_execs {
        work.get_mut(&p).expect("placed").execs.push(e);
    }
    if work.is_empty() || (!check && work.values().all(|w| w.execs.is_empty())) {
        return Commit::Done(then(), Vec::new());
    }
    let opens: Vec<Open> = match work.values().map(|w| all.connection(&w.database)).collect() {
        Ok(c) => c,
        Err(e) => return Commit::Refused(format!("{WRITE_FAILED}{e}")),
    };
    let lim = &permit.databases;
    let mut held: Vec<Held> = Vec::new();
    let undo = |held: &mut Vec<Held>| held.drain(..).for_each(Held::undo);
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    for (w, open) in work.values().zip(&opens) {
        match open {
            Open::Sqlite(conn) => {
                let conn = conn.lock().unwrap();
                let _ = conn.busy_timeout(limits.commit_wait);
                // Held against every other writer from the check to the commit; a database it
                // can't write (read-only media) only reads, and its check is as of now.
                let begun = conn.execute_batch("begin immediate").or_else(|e| if busy(&e) { Err(e) } else { conn.execute_batch("begin") });
                if let Err(e) = begun {
                    undo(&mut held);
                    return if busy(&e) {
                        Commit::Again(format!("another program held the database {:?} past commit_wait_ms", w.name))
                    } else {
                        Commit::Refused(format!("{WRITE_FAILED}the database {:?}: {e}", w.name))
                    };
                }
                held.push(Held::Sqlite(conn));
                let Some(Held::Sqlite(conn)) = held.last() else { unreachable!("pushed") };
                if check {
                    for r in &w.reads {
                        let now = asked(&r.request).and_then(|a| {
                            let mut g = Gather::new(a.sql, a.one, lim.max_db_rows, lim.max_db_read_bytes);
                            select(conn, a.sql, a.params, Instant::now() + lim.db_statement, &mut g).map(|_| answer_of(&a, g.out))
                        });
                        if now.as_ref().ok() != Some(&r.answer) {
                            undo(&mut held);
                            return Commit::Again(format!("a row it read in the database {:?} changed", w.name));
                        }
                    }
                }
                if w.execs.is_empty() {
                    continue;
                }
                if w.database.access != FolderAccess::ReadWrite {
                    undo(&mut held);
                    return Commit::Refused(format!("{WRITE_FAILED}the database {:?} is bound read-only on this server", w.name));
                }
                let done: Result<bool, _> = conn.query_row(&format!("select count(*) from {COMMITS_TABLE} where run = ?1"), [run_id], |row| row.get::<_, i64>(0)).map(|n| n > 0);
                match done {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => {
                        undo(&mut held);
                        return Commit::Refused(format!("{WRITE_FAILED}the database {:?} has no {COMMITS_TABLE} (bind it again): {e}", w.name));
                    }
                }
                for exec in &w.execs {
                    let applied = statement_of(&w.name, exec, permit).and_then(|(sql, params, expect)| {
                        let params: Vec<Sql> = params.iter().map(param).collect::<Result<_, _>>().map_err(|why| format!("{WRITE_FAILED}{why}"))?;
                        let until = Instant::now() + lim.db_statement;
                        let changed = timed(conn, until, |conn| conn.execute(sql, rusqlite::params_from_iter(params.iter()))).map_err(|e| format!("{WRITE_FAILED}{}", said(&e, sql)))?;
                        met(sql, changed as u64, expect)
                    });
                    if let Err(e) = applied {
                        undo(&mut held);
                        return Commit::Refused(e);
                    }
                }
                if let Err(e) = conn.execute(&format!("insert into {COMMITS_TABLE} (run, at) values (?1, ?2)"), rusqlite::params![run_id, at]) {
                    undo(&mut held);
                    return Commit::Refused(format!("{WRITE_FAILED}the database {:?}: {e}", w.name));
                }
            }
            Open::Remote(remote) => {
                // Another server's commit, or a lock it couldn't have in time, sends it round again.
                let failed = |held: &mut Vec<Held>, f: Failure| {
                    undo(held);
                    if f.again { Commit::Again(format!("the database {:?}: {}", w.name, f.why)) } else { Commit::Refused(format!("{WRITE_FAILED}the database {:?}: {}", w.name, f.why)) }
                };
                let mut tx = match remote.begin(limits.commit_wait, lim.db_statement) {
                    Ok(tx) => tx,
                    Err(f) => return failed(&mut held, f),
                };
                if check {
                    for r in &w.reads {
                        let Ok(a) = asked(&r.request) else { continue };
                        let mut g = Gather::new(a.sql, a.one, lim.max_db_rows, lim.max_db_read_bytes);
                        match tx.select(a.sql, a.params, &mut g) {
                            Ok(()) if answer_of(&a, g.out) == r.answer => {}
                            Err(f) if f.again => return failed(&mut held, f),
                            _ => {
                                drop(tx);
                                undo(&mut held);
                                return Commit::Again(format!("a row it read in the database {:?} changed", w.name));
                            }
                        }
                    }
                }
                if !w.execs.is_empty() {
                    if w.database.access != FolderAccess::ReadWrite {
                        drop(tx);
                        undo(&mut held);
                        return Commit::Refused(format!("{WRITE_FAILED}the database {:?} is bound read-only on this server", w.name));
                    }
                    match tx.done(run_id) {
                        Ok(true) => {}
                        Ok(false) => {
                            for exec in &w.execs {
                                let (sql, params, expect) = match statement_of(&w.name, exec, permit) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        drop(tx);
                                        undo(&mut held);
                                        return Commit::Refused(e);
                                    }
                                };
                                let changed = match tx.exec(sql, params) {
                                    Ok(n) => n,
                                    Err(f) => {
                                        drop(tx);
                                        return failed(&mut held, f);
                                    }
                                };
                                if let Err(e) = met(sql, changed, expect) {
                                    drop(tx);
                                    undo(&mut held);
                                    return Commit::Refused(e);
                                }
                            }
                            if let Err(f) = tx.mark(run_id, at) {
                                drop(tx);
                                return failed(&mut held, f);
                            }
                        }
                        Err(f) => {
                            drop(tx);
                            return failed(&mut held, Failure { why: format!("it has no {COMMITS_TABLE} (bind it again): {}", f.why), again: f.again });
                        }
                    }
                }
                held.push(Held::Remote(tx));
            }
        }
    }
    // A serializable transaction can still fail as it commits: these commit first, so one that
    // does while nothing of the run has landed sends it round again with nothing written.
    let mut failed = Vec::new();
    let mut sqlite: Vec<(MutexGuard<Connection>, &str)> = Vec::new();
    let mut committed = false;
    let mut held = held.into_iter().zip(work.values()).collect::<Vec<_>>().into_iter();
    while let Some((h, w)) = held.next() {
        match h {
            Held::Sqlite(c) => sqlite.push((c, &w.name)),
            Held::Remote(tx) => match tx.commit() {
                Ok(()) => committed = true,
                Err(f) if !committed => {
                    sqlite.into_iter().for_each(|(c, _)| Held::Sqlite(c).undo());
                    held.for_each(|(h, _)| h.undo());
                    return if f.again {
                        Commit::Again(format!("the database {:?}: {}", w.name, f.why))
                    } else {
                        Commit::Refused(format!("{WRITE_FAILED}the database {:?}: {}", w.name, f.why))
                    };
                }
                Err(f) => failed.push(format!("the database {:?} didn't commit: {}", w.name, f.why)),
            },
        }
    }
    let out = then();
    failed.extend(sqlite.iter().filter_map(|(c, name)| c.execute_batch("commit").err().map(|e| format!("the database {name:?} didn't commit: {e}"))));
    Commit::Done(out, failed)
}

/// One statement a run made (`{sql, params, expect}`), its tables checked against the grant
/// again: its text, parameters and `expect`.
fn statement_of<'e>(name: &str, exec: &'e Value, permit: &Permit) -> Result<(&'e str, &'e [Value], Option<f64>), String> {
    let Value::Record(e) = exec else { return Err(format!("{WRITE_FAILED}a statement is a record")) };
    let (Some(Value::Text(sql)), Some(Value::List(params))) = (e.get("sql"), e.get("params")) else {
        return Err(format!("{WRITE_FAILED}a statement has its sql and params"));
    };
    let tables = holon_core::sql::tables(sql, true).map_err(|why| format!("{WRITE_FAILED}{sql:?}: {why}"))?;
    let reads = tables.reads.iter().map(|t| Capability::DbRead(format!("{name}/{t}")));
    let writes = tables.writes.iter().map(|t| Capability::DbWrite(format!("{name}/{t}")));
    if let Some(c) = reads.chain(writes).find(|c| !permit.allows(c)) {
        return Err(format!("{WRITE_FAILED}{sql:?} needs {c:?}, which isn't granted to this deployment"));
    }
    let expect = match e.get("expect") {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    };
    Ok((sql, params, expect))
}

/// Whether a statement changed the rows it expects: why not, otherwise.
fn met(sql: &str, changed: u64, expect: Option<f64>) -> Result<(), String> {
    match expect {
        Some(n) if n as u64 != changed => {
            let rows = |k: u64| if k == 1 { "1 row".to_string() } else { format!("{k} rows") };
            Err(format!("{UNMET}{sql:?} changed {}, not {}", rows(changed), rows(n as u64)))
        }
        _ => Ok(()),
    }
}

// ── admin ──

fn shown(name: &str, deployment: Option<&str>, d: &Database) -> serde_json::Value {
    let mut shown = serde_json::json!({ "name": name, "deployment": deployment, "engine": d.engine, "access": d.access, "lock": d.lock });
    match (&d.connection, &d.address) {
        (Some(source), address) => {
            shown["connection"] = serde_json::json!(source);
            shown["address"] = serde_json::json!(address);
        }
        _ => shown["path"] = serde_json::json!(d.path.display().to_string()),
    }
    shown
}

fn labelled(name: &str, deployment: Option<&str>) -> String {
    deployment.map_or_else(|| name.to_string(), |d| format!("{name} (for {d})"))
}

/// `/admin/databases` (list, bind), `/admin/databases/<name>` (unbind; `?deployment=<slug>` for
/// a deployment's own), or None for any other path. Only an admin gets this far (access.rs); what
/// they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    let all = &state.folders.databases;
    if url == "/admin/databases" {
        return Some(match method {
            "GET" => {
                let bound = all.all();
                let server = bound.server.iter().map(|(n, d)| shown(n, None, d));
                let own = bound.deployments.iter().flat_map(|(s, dbs)| dbs.iter().map(move |(n, d)| shown(n, Some(s), d)));
                Response::json(&server.chain(own).collect::<Vec<_>>())
            }
            "POST" => {
                #[derive(Deserialize)]
                struct Bind {
                    name: String,
                    /// A SQLite database's file.
                    #[serde(default)]
                    path: Option<String>,
                    /// Or where a Postgres or MySQL database's URL is fetched from.
                    #[serde(default)]
                    connection: Option<String>,
                    #[serde(default = "read_write")]
                    access: FolderAccess,
                    /// The admin's DDL, run first (and the file created, if there is none).
                    #[serde(default)]
                    schema: Option<String>,
                    /// Its runs hold it for their whole length.
                    #[serde(default)]
                    lock: locks::Setting,
                    #[serde(default)]
                    deployment: Option<String>,
                }
                fn read_write() -> FolderAccess {
                    FolderAccess::ReadWrite
                }
                let bound = read_body(request)
                    .and_then(|b| serde_json::from_slice::<Bind>(&b).map_err(|e| err(400, format!("invalid body: {e}"))))
                    .map(|b| Bind { deployment: b.deployment.filter(|d| !d.trim().is_empty()), ..b })
                    .and_then(|b| {
                        let target = b.connection.as_deref().filter(|c| !c.trim().is_empty()).or(b.path.as_deref()).ok_or_else(|| err(400, "a database is a path (SQLite) or a connection (env:… or vault:…)"))?;
                        let d = all.bind_as(&b.name, target, b.access, b.schema.as_deref(), b.lock, b.deployment.as_deref()).map_err(|e| err(400, e))?;
                        Ok((b, d))
                    });
                match bound {
                    Ok((b, d)) => {
                        state.activity.record("bound the database", &labelled(&b.name, b.deployment.as_deref()), Some(by));
                        Response::json(&shown(&b.name, b.deployment.as_deref(), &d))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    let name = url.strip_prefix("/admin/databases/")?;
    let deployment = request.get_param("deployment").filter(|d| !d.is_empty());
    if method != "DELETE" {
        return Some(Response::empty_404());
    }
    Some(match all.unbind(name, deployment.as_deref()) {
        Ok(true) => {
            state.activity.record("unbound the database", &labelled(name, deployment.as_deref()), Some(by));
            Response::empty_204()
        }
        Ok(false) => err(404, format!("no database {name:?} is bound{}", deployment.map_or(String::new(), |d| format!(" for {d}")))),
        Err(e) => err(500, e),
    })
}

/// `holon database add NAME PATH [--read-only] [--schema SQL | --schema-file FILE] [--lock run] [--for SLUG] |
/// list | remove NAME`: bind, show and unbind the databases of the server this user runs here (or
/// `--server`), as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut for_deployment: Option<String> = None;
    let mut read_only = false;
    let mut lock = locks::Setting::Commit;
    let mut schema: Option<String> = None;
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--for" => for_deployment = args.next().cloned(),
            "--read-only" => read_only = true,
            "--lock" => match args.next().map(String::as_str) {
                Some("run") => lock = locks::Setting::Run,
                Some("commit") => lock = locks::Setting::Commit,
                _ => {
                    eprintln!("holon database: --lock takes run (its runs hold it for their whole length) or commit");
                    return 2;
                }
            },
            "--schema" => schema = args.next().cloned(),
            "--schema-file" => match args.next().map(std::fs::read_to_string) {
                Some(Ok(s)) => schema = Some(s),
                Some(Err(e)) => {
                    eprintln!("holon database: --schema-file: {e}");
                    return 1;
                }
                None => {
                    eprintln!("holon database: --schema-file takes a file of SQL");
                    return 2;
                }
            },
            "--help" | "-h" => {
                println!("{DATABASE_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon database: no option {a:?}\n\n{DATABASE_USAGE}");
                return 2;
            }
        }
    }
    let (method, path, body) = match words.as_slice() {
        ["list"] => ("GET", "/admin/databases".to_string(), None),
        ["add", name, target] => {
            let access = if read_only { FolderAccess::Read } else { FolderAccess::ReadWrite };
            let mut body = serde_json::json!({ "name": name, "access": access, "schema": schema, "lock": lock, "deployment": for_deployment });
            if target.starts_with("env:") || target.starts_with("vault:") {
                body["connection"] = serde_json::json!(target);
            } else {
                body["path"] = serde_json::json!(std::path::absolute(target).map_or_else(|_| target.to_string(), |p| p.display().to_string()));
            }
            ("POST", "/admin/databases".to_string(), Some(body))
        }
        ["remove", name] => {
            let only = for_deployment.as_ref().map_or(String::new(), |d| format!("?deployment={d}"));
            ("DELETE", format!("/admin/databases/{name}{only}"), None)
        }
        _ => {
            eprintln!("{DATABASE_USAGE}");
            return 2;
        }
    };
    let (server, answer) = match local::admin_call("holon database", server.as_deref(), method, &path, body.as_ref()) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let line = |d: &serde_json::Value| {
        let access = if d["access"] == "read" { "read only" } else { "read and write" };
        let only = d["deployment"].as_str().map_or(String::new(), |s| format!(", for {s} only"));
        let at = match d["connection"].as_str() {
            Some(source) => format!("{} (from {source})", d["address"].as_str().unwrap_or("")),
            None => d["path"].as_str().unwrap_or("").to_string(),
        };
        let lock = if d["lock"] == "run" { ", held for whole runs" } else { "" };
        format!("{}  {} {at}  ({access}{lock}{only})", d["name"].as_str().unwrap_or(""), d["engine"].as_str().unwrap_or(""))
    };
    match words[0] {
        "list" => {
            let all: Vec<serde_json::Value> = serde_json::from_str(&answer).unwrap_or_default();
            if all.is_empty() {
                println!("no databases are bound on {server}: holon database add NAME PATH|env:… binds one");
            }
            for d in &all {
                println!("{}", line(d));
            }
        }
        "add" => println!("bound {}", line(&serde_json::from_str(&answer).unwrap_or_default())),
        _ => println!(
            "unbound {}{}: the database stays where it is",
            words[1],
            for_deployment.as_ref().map_or(String::new(), |d| format!(" (for {d})"))
        ),
    }
    0
}

const DATABASE_USAGE: &str = "usage: holon database add NAME PATH|SOURCE [--read-only] [--schema SQL | --schema-file FILE] [--lock run] [--for SLUG]
       holon database list
       holon database remove NAME [--for SLUG]
       (each takes --server PORT|URL)

  Binds a name a flow uses (database shop) to a database, for an admin: a SQLite database's file
  on the server's machine (PATH), or a Postgres or MySQL database whose URL the server fetches
  from SOURCE, as a secret's: env:HOLON_SECRET_SHOP_URL (the server's environment or --dotenv)
  or vault:<mount>/<path>#<field>. The URL (postgres://user:password@host/db, mysql://…) is never
  stored, shown or logged; ?sslmode= (Postgres) and ?ssl-mode= (MySQL) choose TLS. A flow reads
  and writes only the tables it was granted, by the SQL it is written with; it never sees where
  the database is, and runs no DDL.
  --schema runs the admin's SQL (create table …) when it is bound, creating a SQLite file if
  there is none; without it, PATH must be a SQLite database already. --read-only: flows may read
  it, not write it. A writable database gets a table holon_commits, where each run's commit
  marks it done, so a run is never applied twice (and on MySQL a table holon_claims, whose rows'
  locks are the claims of several servers). --lock run: each run that touches it holds it from
  before it starts until it commits, fails or waits, shared if it only reads it, alone if it
  may write it, so every run that writes it goes one at a time, for its whole length, across
  every deployment bound to it (and, on Postgres or MySQL, every server); one that waits past
  its claim_wait is turned away (503). For what a commit's check can't cover: a sequence's
  next value, a queue, or contention so high that running again costs more than waiting.
  --for SLUG binds it for that deployment only.
  Removing a binding leaves the database where it is; deployments that use it stop until it is
  bound again.";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-db-{tag}-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn values_cross_as_their_columns_say() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("create table t (i integer, r real, s text, b blob, n integer, big integer)").unwrap();
        conn.execute("insert into t values (?1, ?2, ?3, ?4, ?5, ?6)", rusqlite::params![7, 2.5, "x", vec![1u8, 255], None::<i64>, i64::MAX]).unwrap();
        let soon = || Instant::now() + Duration::from_secs(5);
        let mut g = Gather::new("select * from t", true, 10, 1 << 20);
        select(&conn, "select * from t", &[], soon(), &mut g).unwrap();
        let Value::Record(row) = &g.out[0] else { panic!() };
        assert_eq!(row.get("i"), Some(&Value::Number(7.0)));
        assert_eq!(row.get("r"), Some(&Value::Number(2.5)));
        assert_eq!(row.get("b"), Some(&Value::Text("01ff".into())));
        assert_eq!(row.get("n"), None, "null is no field");
        assert_eq!(row.get("big"), Some(&Value::Text(i64::MAX.to_string())), "past 2^53, its digits");
        assert_eq!(param(&Value::Boolean(true)).unwrap(), Sql::Integer(1));
        assert_eq!(param(&Value::Number(1.5)).unwrap(), Sql::Real(1.5));
        assert!(select(&conn, "delete from t", &[], soon(), &mut Gather::new("delete from t", false, 10, 1 << 20)).unwrap_err().contains("writes"));
        conn.execute_batch("insert into t (i) values (1), (2)").unwrap();
        let many = "select * from t, t as u, t as v";
        assert!(select(&conn, many, &[], soon(), &mut Gather::new(many, false, 5, 1 << 20)).unwrap_err().contains("max_db_rows"));
        let slow = "with recursive c(n) as (select 1 union all select n + 1 from c) select count(*) from c";
        assert!(select(&conn, slow, &[], Instant::now() + Duration::from_millis(50), &mut Gather::new(slow, true, 5, 1 << 20)).unwrap_err().contains("db_statement_ms"));
    }

    #[test]
    fn a_database_is_bound_by_an_admin_with_its_schema_and_never_inside_the_server() {
        let dir = temp("bind");
        let own = dir.join("data");
        std::fs::create_dir_all(&own).unwrap();
        let dbs = Databases::open(":memory:", std::slice::from_ref(&own)).unwrap();
        let file = dir.join("shop.db");
        let path = file.to_str().unwrap();
        assert!(dbs.bind("shop", path, FolderAccess::ReadWrite, None, None).unwrap_err().contains("give a schema"));
        dbs.bind("shop", path, FolderAccess::ReadWrite, Some("create table orders (id integer primary key)"), None).unwrap();
        let conn = Connection::open(&file).unwrap();
        let tables: Vec<String> = conn.prepare("select name from sqlite_master where type = 'table' order by name").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(tables, ["holon_commits", "orders"]);
        assert!(dbs.bind("x", own.join("a.db").to_str().unwrap(), FolderAccess::Read, Some("create table t (a)"), None).unwrap_err().contains("own data"));
        assert!(dbs.bind("x", "shop.db", FolderAccess::Read, None, None).unwrap_err().contains("absolute"));
        std::fs::write(dir.join("not.db"), "hello, this is text and not a database at all, really").unwrap();
        assert!(dbs.bind("x", dir.join("not.db").to_str().unwrap(), FolderAccess::Read, None, None).unwrap_err().contains("isn't a SQLite database"));
        assert!(dbs.bind("x", path, FolderAccess::Read, Some("create tabel"), None).unwrap_err().contains("schema didn't run"));
        assert_eq!(dbs.resolve("any", "shop").unwrap().access, FolderAccess::ReadWrite);
        dbs.bind("shop", path, FolderAccess::Read, None, Some("billing")).unwrap();
        assert_eq!(dbs.resolve("billing", "shop").unwrap().access, FolderAccess::Read, "a deployment's own binding wins");
        assert!(dbs.unbind("shop", Some("billing")).unwrap());
        drop((conn, dbs));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A read that pins a table's whole primary key claims its row; any other, the table; an
    /// insert claims the row it names, or a new row (docs/L4s/databases-L4.md §7.2). A table the
    /// deployment can't write is never claimed.
    #[test]
    fn a_primary_key_read_claims_its_row_and_any_other_the_table() {
        let dir = temp("claims");
        let dbs = Databases::open(":memory:", &[]).unwrap();
        let file = dir.join("shop.db");
        let schema = "create table orders (id integer primary key, customer text); create table lines (order_id integer, n integer, primary key (order_id, n)); create table notes (text text)";
        dbs.bind("shop", file.to_str().unwrap(), FolderAccess::ReadWrite, Some(schema), None).unwrap();
        let folders = folders::Scoped::new(Arc::new(folders::Folders::memory().with_databases(dbs)), "d");
        let written: BTreeSet<String> = ["shop/orders", "shop/lines", "shop/notes"].map(String::from).into();
        let read = |sql: &str, params: Vec<Value>| -> Vec<String> {
            let mut request = BTreeMap::new();
            request.insert("database".to_string(), Value::Text("shop".into()));
            request.insert("query".to_string(), Value::Text(sql.into()));
            request.insert("params".to_string(), Value::List(params));
            claims_of_read(&folders, &Value::Record(request), &written).iter().map(ToString::to_string).collect()
        };
        let n = |x: f64| Value::Number(x);
        assert_eq!(read("select * from orders where id = $1 and customer = $2", vec![Value::Text("7".into()), Value::Text("ada".into())]), ["shop/orders[7]"]);
        assert_eq!(read("select * from orders where id = $1", vec![n(7.0)]), ["shop/orders[7]"], "7 and \"7\" are one row");
        assert_eq!(read("select * from orders where customer = $1", vec![Value::Text("ada".into())]), ["shop/orders[*]"]);
        assert_eq!(read("select * from orders where id > $1", vec![n(1.0)]), ["shop/orders[*]"]);
        assert_eq!(read("select * from lines where order_id = $1 and n = $2", vec![n(7.0), n(2.0)]), ["shop/lines[7,2]"]);
        assert_eq!(read("select * from lines where order_id = $1", vec![n(7.0)]), ["shop/lines[*]"], "half a key is the table");
        assert_eq!(read("select * from notes where rowid = $1", vec![n(1.0)]), ["shop/notes[*]"], "no primary key: the table");
        assert_eq!(read("select * from orders o join lines l on l.order_id = o.id where o.id = $1", vec![n(7.0)]), ["shop/lines[*]", "shop/orders[*]"]);
        let only_orders: BTreeSet<String> = ["shop/orders".to_string()].into();
        let mut request = BTreeMap::new();
        request.insert("database".to_string(), Value::Text("shop".into()));
        request.insert("query".to_string(), Value::Text("select * from orders o join lines l on l.order_id = o.id".into()));
        request.insert("params".to_string(), Value::List(vec![]));
        let keys: Vec<String> = claims_of_read(&folders, &Value::Record(request), &only_orders).iter().map(ToString::to_string).collect();
        assert_eq!(keys, ["shop/orders[*]"], "a table it can't write is never claimed");
        let exec = |sql: &str, params: Vec<Value>| -> Vec<String> {
            let mut payload = BTreeMap::new();
            payload.insert("sql".to_string(), Value::Text(sql.into()));
            payload.insert("params".to_string(), Value::List(params));
            claims_of_writes(&folders, &[(Capability::DbWrite("shop/orders".into()), Value::Record(payload))]).iter().map(ToString::to_string).collect()
        };
        assert_eq!(exec("update orders set customer = $2 where id = $1", vec![n(7.0), Value::Text("b".into())]), ["shop/orders[7]"]);
        assert_eq!(exec("insert into orders (id, customer) values ($1, $2)", vec![n(9.0), Value::Text("b".into())]), ["shop/orders[9]"]);
        assert_eq!(exec("insert into orders (customer) values ($1)", vec![Value::Text("b".into())]), ["shop/orders[new]"]);
        assert_eq!(exec("delete from orders where customer = $1", vec![Value::Text("b".into())]), ["shop/orders[*]"]);
        drop(folders);
        let _ = std::fs::remove_dir_all(dir);
    }
}
