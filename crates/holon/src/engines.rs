//! The Postgres and MySQL databases an admin binds (docs/L4s/databases-L4.md §8): a pool of
//! connections to each, opened with the connection the admin named (a secret source, fetched by
//! the host and never logged), and what databases.rs asks of them.
//!
//! - **Reads** run each in a read-only transaction, within `db_statement_ms`. Parameters go to
//!   Postgres as text, so the server parses each as the type its column has (`7` for an integer,
//!   `"12.50"` for a numeric); MySQL takes `?`, so `$n` is rewritten (core's `sql::positional`).
//! - **Values:** text, numbers, true/false and none as they read; exact decimals (`numeric`,
//!   `decimal`) as text, so a total is never rounded; timestamps as RFC 3339 text (UTC); JSON as a
//!   value; bytes as hex. A column of another type fails the read, saying to cast it.
//! - **A commit** is one transaction, `serializable` on both. On MySQL that makes each read of the
//!   check a locking read (InnoDB's `for share`, ranges included), so what it checked holds, against
//!   any writer, until it commits. On Postgres it is serializable snapshot isolation: two Holon
//!   commits that would each have missed the other's write can't both commit, and a run whose read
//!   changed before its check sees it there; `for share` would need UPDATE rights on every table a
//!   run reads, which a role granted only what it uses doesn't have. A deadlock, a serialization
//!   failure or a lock not had within `commit_wait_ms` sends the run round again.
//! - **Claims for several servers** (§7.2) are held in the database, on a connection of their
//!   own, so a server that stops lets go with its connection: Postgres advisory locks (a row's
//!   table shared and the row exclusive, a whole table exclusive), and on MySQL, which has no shared
//!   `GET_LOCK`, record locks on rows of a `holon_claims` table, `for share` and `for update`.
//!   A database an admin set to `run` (§7.3) has one more such lock, its run lock, which runs that
//!   touch it hold for their whole length: shared to read, exclusive to write.
//! - **Each session** reads strings as standard SQL does: Postgres with
//!   `standard_conforming_strings`, MySQL with `ANSI_QUOTES`, `PIPES_AS_CONCAT` and
//!   `NO_BACKSLASH_ESCAPES`, so the engine reads a statement as core's parser did when it found its
//!   tables. Times are UTC.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use holon_core::Value;
use postgres::fallible_iterator::FallibleIterator;
use postgres::types::{FromSql, Format, IsNull, Kind, ToSql, Type, to_sql_checked};
use zeroize::Zeroizing;

use crate::claims::ClaimKey;
use crate::databases::{COMMITS_TABLE, Gather};

/// The table on a MySQL database whose rows' locks are the claims several servers hold (and the
/// collection on a MongoDB one whose documents are: mongo.rs).
pub(crate) const CLAIMS_TABLE: &str = "holon_claims";

/// How long an idle connection is trusted without being asked whether it is still there.
const FRESH: Duration = Duration::from_secs(30);
/// Idle connections kept per database.
const IDLE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    Postgres,
    Mysql,
}

impl Engine {
    /// The engine a connection URL is for, by its scheme.
    pub(crate) fn of(url: &str) -> Option<Engine> {
        match url.split_once("://")?.0.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Some(Engine::Postgres),
            "mysql" => Some(Engine::Mysql),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::Mysql => "mysql",
        }
    }
}

/// Why a statement failed, for the log; `again` when it was two transactions meeting (a deadlock,
/// a serialization failure, a lock not had in time), so the run goes round again.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) why: String,
    pub(crate) again: bool,
}

impl Failure {
    fn said(why: impl Into<String>) -> Failure {
        Failure { why: why.into(), again: false }
    }
}

enum Conn {
    Pg(Box<postgres::Client>),
    My(mysql::Conn),
}

/// One bound Postgres or MySQL database: where it is, and its connections.
pub(crate) struct Remote {
    pub(crate) engine: Engine,
    /// Where it is, without credentials (`postgres://host:5432/shop`): what claims and commit
    /// locks know it by, so two bindings of one database are one.
    pub(crate) address: String,
    url: Zeroizing<String>,
    idle: Mutex<Vec<(Conn, Instant)>>,
    /// The claims held in it for several servers, by holder (a run): each on its own connection.
    sessions: Mutex<BTreeMap<String, Arc<Mutex<Session>>>>,
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote").field("address", &self.address).finish()
    }
}

/// Where `url` points, without its credentials: `postgres://host:port/database`.
pub(crate) fn address_of(url: &str) -> Result<(Engine, String), String> {
    let engine = Engine::of(url).ok_or("a connection is a URL: postgres://… or mysql://…")?;
    let address = match engine {
        Engine::Postgres => {
            let config: postgres::Config = url.parse().map_err(|e| format!("the connection isn't a Postgres URL: {e}"))?;
            let host = match config.get_hosts().first() {
                Some(postgres::config::Host::Tcp(h)) => h.to_lowercase(),
                // A Unix socket's folder (`host=/run/postgresql`), a host there is only on Unix.
                #[cfg(unix)]
                Some(postgres::config::Host::Unix(path)) => path.display().to_string(),
                None => "localhost".to_string(),
            };
            let port = config.get_ports().first().copied().unwrap_or(5432);
            let db = config.get_dbname().or(config.get_user()).unwrap_or("postgres");
            format!("postgres://{host}:{port}/{db}")
        }
        Engine::Mysql => {
            let (url, _) = mysql_tls(url)?;
            let opts = mysql::Opts::from_url(&url).map_err(|e| format!("the connection isn't a MySQL URL: {e}"))?;
            format!("mysql://{}:{}/{}", opts.get_ip_or_hostname().to_lowercase(), opts.get_tcp_port(), opts.get_db_name().unwrap_or(""))
        }
    };
    Ok((engine, address))
}

/// A MySQL URL without its `ssl-mode` (which the client library doesn't take), and whether to
/// connect with TLS: as `ssl-mode` says, else unless the server is this machine.
fn mysql_tls(url: &str) -> Result<(String, bool), String> {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut mode = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| match p.split_once('=') {
            Some((k, v)) if k.eq_ignore_ascii_case("ssl-mode") || k.eq_ignore_ascii_case("sslmode") => {
                mode = Some(v.to_ascii_uppercase());
                false
            }
            _ => true,
        })
        .collect();
    let url = if rest.is_empty() { base.to_string() } else { format!("{base}?{}", rest.join("&")) };
    let tls = match mode.as_deref() {
        Some("DISABLED" | "DISABLE") => false,
        Some("REQUIRED" | "REQUIRE" | "VERIFY_CA" | "VERIFY_IDENTITY" | "VERIFY-FULL") => true,
        Some(other) => return Err(format!("ssl-mode={other} isn't one Holon knows: DISABLED or REQUIRED")),
        None => {
            let host = mysql::Opts::from_url(&url).map(|o| o.get_ip_or_hostname().to_lowercase()).unwrap_or_default();
            !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
        }
    };
    Ok((url, tls))
}

fn pg_tls() -> tokio_postgres_rustls::MakeRustlsConnect {
    static CONFIG: OnceLock<rustls::ClientConfig> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring has TLS 1.2 and 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth()
    });
    tokio_postgres_rustls::MakeRustlsConnect::new(config.clone())
}

impl Remote {
    /// The database at `url`, reached once now, so a binding that can't connect says so.
    pub(crate) fn open(url: Zeroizing<String>) -> Result<Remote, String> {
        let (engine, address) = address_of(&url)?;
        let remote = Remote { engine, address, url, idle: Mutex::new(Vec::new()), sessions: Mutex::new(BTreeMap::new()) };
        drop(remote.conn()?);
        Ok(remote)
    }

    fn connect(&self) -> Result<Conn, String> {
        let unreachable = |e: &dyn std::fmt::Display| format!("{} can't be reached: {e}", self.address);
        match self.engine {
            Engine::Postgres => {
                let mut config: postgres::Config = self.url.parse().map_err(|e| format!("the connection isn't a Postgres URL: {e}"))?;
                config.connect_timeout(Duration::from_secs(5)).application_name("holon");
                let mut client = config.connect(pg_tls()).map_err(|e| unreachable(&e))?;
                client.batch_execute("set standard_conforming_strings = on; set timezone = 'UTC'").map_err(|e| unreachable(&e))?;
                Ok(Conn::Pg(Box::new(client)))
            }
            Engine::Mysql => {
                let (url, tls) = mysql_tls(&self.url)?;
                let opts = mysql::Opts::from_url(&url).map_err(|e| format!("the connection isn't a MySQL URL: {e}"))?;
                let mut builder = mysql::OptsBuilder::from_opts(opts)
                    .tcp_connect_timeout(Some(Duration::from_secs(5)))
                    // `expect` counts the rows a statement matched, as the other engines do.
                    .additional_capabilities(mysql::consts::CapabilityFlags::CLIENT_FOUND_ROWS)
                    .init(vec![
                        "set session sql_mode = concat_ws(',', nullif(@@session.sql_mode, ''), 'ANSI_QUOTES', 'PIPES_AS_CONCAT', 'NO_BACKSLASH_ESCAPES')",
                        "set session time_zone = '+00:00'",
                    ]);
                if tls {
                    builder = builder.ssl_opts(mysql::SslOpts::default());
                }
                mysql::Conn::new(builder).map(Conn::My).map_err(|e| unreachable(&e))
            }
        }
    }

    /// A connection: an idle one (asked whether it is still there if it has been idle a while),
    /// else a new one.
    fn conn(&self) -> Result<Pooled<'_>, String> {
        loop {
            let next = self.idle.lock().unwrap().pop();
            let Some((mut conn, since)) = next else { break };
            let fine = since.elapsed() < FRESH
                || match &mut conn {
                    Conn::Pg(c) => c.is_valid(Duration::from_secs(2)).is_ok(),
                    Conn::My(c) => c.ping().is_ok(),
                };
            if fine && !matches!(&conn, Conn::Pg(c) if c.is_closed()) {
                return Ok(Pooled { remote: self, conn: Some(conn), broken: false });
            }
        }
        Ok(Pooled { remote: self, conn: Some(self.connect()?), broken: false })
    }

    fn put_back(&self, conn: Conn) {
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < IDLE {
            idle.push((conn, Instant::now()));
        }
    }

    /// Run the admin's DDL (`--schema`), in one transaction where the engine has DDL in one.
    pub(crate) fn run_schema(&self, schema: &str) -> Result<(), String> {
        let mut c = self.conn()?;
        let done = match c.inner() {
            Conn::Pg(p) => p.batch_execute(&format!("begin; {schema}; commit;")).map_err(|e| {
                let _ = p.batch_execute("rollback");
                pg_said(&e)
            }),
            Conn::My(m) => mysql::prelude::Queryable::query_drop(m, schema).map_err(|e| my_said(&e)),
        };
        done.map_err(|e| format!("the schema didn't run: {e}"))
    }

    /// Make the tables Holon keeps in a database it writes: `holon_commits`, and on MySQL the
    /// claims' rows.
    pub(crate) fn keep_tables(&self) -> Result<(), String> {
        let mut c = self.conn()?;
        let kept = match c.inner() {
            Conn::Pg(p) => p.batch_execute(&format!("create table if not exists {COMMITS_TABLE} (run text primary key, at bigint not null)")).map_err(|e| pg_said(&e)),
            Conn::My(m) => {
                use mysql::prelude::Queryable;
                m.query_drop(format!("create table if not exists {COMMITS_TABLE} (run varchar(64) primary key, at bigint not null)"))
                    .and_then(|_| m.query_drop(format!("create table if not exists {CLAIMS_TABLE} (k char(32) primary key)")))
                    .map_err(|e| my_said(&e))
            }
        };
        kept.map_err(|e| format!("can't keep {COMMITS_TABLE} in it: {e}"))
    }

    /// The columns of `table`'s primary key, in order and in lower case; none for a table without
    /// one, or one that isn't there.
    pub(crate) fn primary_key(&self, table: &str) -> Option<Vec<String>> {
        let mut c = self.conn().ok()?;
        let columns: Vec<String> = match c.inner() {
            Conn::Pg(p) => {
                let sql = "select lower(a.attname::text) from pg_index i join pg_attribute a on a.attrelid = i.indrelid and a.attnum = any(i.indkey) \
                           where i.indrelid = to_regclass($1) and i.indisprimary order by array_position(i.indkey::int2[], a.attnum)";
                p.query(sql, &[&Text(Some(quoted(table)))]).ok()?.iter().map(|r| r.get::<_, String>(0)).collect()
            }
            Conn::My(m) => {
                use mysql::prelude::Queryable;
                let (schema, name) = table.split_once('.').map_or((None, table), |(s, t)| (Some(s), t));
                let sql = "select lower(column_name) from information_schema.key_column_usage where constraint_name = 'PRIMARY' \
                           and table_schema = coalesce(?, database()) and table_name = ? order by ordinal_position";
                m.exec(sql, (schema, name)).ok()?
            }
        };
        (!columns.is_empty()).then_some(columns)
    }

    /// The rows a read selects, gathered into `g`, in a read-only transaction within `statement`.
    pub(crate) fn read(&self, sql: &str, params: &[Value], g: &mut Gather, statement: Duration) -> Result<(), String> {
        let mut c = self.conn()?;
        let ms = statement.as_millis().max(1);
        let read = match c.inner() {
            Conn::Pg(p) => p
                .batch_execute(&format!("begin read only; set local statement_timeout = {ms}"))
                .map_err(|e| pg_failed(&e, sql))
                .and_then(|_| pg_query(p, sql, params, g))
                .and_then(|_| p.batch_execute("commit").map_err(|e| pg_failed(&e, sql))),
            Conn::My(m) => {
                use mysql::prelude::Queryable;
                m.query_drop(format!("set session max_execution_time = {ms}"))
                    .and_then(|_| m.query_drop("start transaction read only"))
                    .map_err(|e| my_failed(&e, sql))
                    .and_then(|_| my_query(m, sql, params, g))
                    .and_then(|_| m.query_drop("commit").map_err(|e| my_failed(&e, sql)))
            }
        };
        if read.is_err() && !c.rollback() {
            c.broken = true;
        }
        read.map_err(|f| f.why)
    }

    /// A commit's transaction, its locks waited for `commit_wait` at most and each statement
    /// `statement` at most.
    pub(crate) fn begin(&self, commit_wait: Duration, statement: Duration) -> Result<Txn<'_>, Failure> {
        let mut c = self.conn().map_err(Failure::said)?;
        let ms = statement.as_millis().max(1);
        let begun = match c.inner() {
            Conn::Pg(p) => p
                .batch_execute(&format!(
                    "begin isolation level serializable; set local lock_timeout = {}; set local statement_timeout = {ms}",
                    commit_wait.as_millis().max(1)
                ))
                .map_err(|e| pg_failed(&e, "begin")),
            Conn::My(m) => {
                use mysql::prelude::Queryable;
                let wait = commit_wait.as_secs_f64().ceil().max(1.0) as u64;
                [
                    format!("set session innodb_lock_wait_timeout = {wait}"),
                    format!("set session max_execution_time = {ms}"),
                    "set transaction isolation level serializable".to_string(),
                    "start transaction".to_string(),
                ]
                .iter()
                .try_for_each(|s| m.query_drop(s))
                .map_err(|e| my_failed(&e, "begin"))
            }
        };
        begun?;
        Ok(Txn { conn: c, open: true })
    }

    // ── claims for several servers ──

    /// Hold `keys` (rows, tables and new rows of this database) for `holder` in the database
    /// itself, all or none, without waiting: the first another server's run holds, otherwise. A
    /// holder taking a key it holds already takes it again (and lets go of it as many times).
    pub(crate) fn claim(&self, keys: &[ClaimKey], holder: &str) -> Result<(), ClaimKey> {
        let locks: Vec<(usize, Lock)> = keys.iter().enumerate().flat_map(|(i, k)| locks_of(k).into_iter().map(move |l| (i, l))).collect();
        self.hold(&locks, holder).map_err(|i| keys[i].clone())
    }

    /// Hold this database's run lock (§7.3) for `holder`, shared or `exclusive`, without waiting:
    /// whether it was free.
    pub(crate) fn hold_run(&self, holder: &str, exclusive: bool) -> bool {
        self.hold(&[(0, (RUN_LOCK.to_string(), exclusive))], holder).is_ok()
    }

    /// Let go of its run lock held for `holder`.
    pub(crate) fn let_go_run(&self, holder: &str, exclusive: bool) {
        self.let_go(&[(RUN_LOCK.to_string(), exclusive)], holder);
    }

    /// Hold `locks` for `holder`, all or none, without waiting: the index (its key's) of the first
    /// another holder has, otherwise.
    fn hold(&self, locks: &[(usize, Lock)], holder: &str) -> Result<(), usize> {
        if locks.is_empty() {
            return Ok(());
        }
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get(holder) {
                Some(s) => Arc::clone(s),
                None => {
                    let conn = self.conn().map_err(|e| {
                        eprintln!("holon: can't claim in {}: {e}", self.address);
                        locks[0].0
                    })?;
                    let s = Arc::new(Mutex::new(Session { conn: conn.into_conn(), held: BTreeMap::new(), began: false }));
                    sessions.insert(holder.to_string(), Arc::clone(&s));
                    s
                }
            }
        };
        let mut s = session.lock().unwrap();
        let mut taken: Vec<&Lock> = Vec::new();
        for (key, lock) in locks {
            if s.held.get(lock).copied().unwrap_or(0) == 0 {
                match s.take(self, lock) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => {
                        for l in &taken {
                            s.drop_one(l);
                        }
                        let empty = s.held.is_empty();
                        drop(s);
                        if empty {
                            self.end_session(holder);
                        }
                        return Err(*key);
                    }
                }
            }
            *s.held.entry(lock.clone()).or_insert(0) += 1;
            taken.push(lock);
        }
        Ok(())
    }

    /// Let go of `keys` held for `holder` (once each), and of its connection once it holds none.
    pub(crate) fn unclaim(&self, keys: &[ClaimKey], holder: &str) {
        let locks: Vec<Lock> = keys.iter().flat_map(locks_of).collect();
        self.let_go(&locks, holder);
    }

    /// Let go of `locks` held for `holder` (once each), and of its connection once it holds none.
    fn let_go(&self, locks: &[Lock], holder: &str) {
        let Some(session) = self.sessions.lock().unwrap().get(holder).map(Arc::clone) else { return };
        let mut s = session.lock().unwrap();
        for lock in locks {
            if s.held.contains_key(lock) {
                s.drop_one(lock);
            }
        }
        let empty = s.held.is_empty();
        drop(s);
        if empty {
            self.end_session(holder);
        }
    }

    fn end_session(&self, holder: &str) {
        let Some(session) = self.sessions.lock().unwrap().remove(holder) else { return };
        let Ok(session) = Arc::try_unwrap(session) else { return };
        let mut s = session.into_inner().unwrap();
        let clean = match &mut s.conn {
            Conn::Pg(p) => p.batch_execute("select pg_advisory_unlock_all()").is_ok(),
            Conn::My(m) => !s.began || mysql::prelude::Queryable::query_drop(m, "rollback").is_ok(),
        };
        if clean {
            self.put_back(s.conn);
        }
    }
}

/// One lock in the database a claim takes: a name (the table, or the table and a row's key) and
/// whether it is exclusive.
pub(crate) type Lock = (String, bool);

/// The name of a database's run lock (§7.3): one no table's name is, so it is no claim's.
pub(crate) const RUN_LOCK: &str = "\u{0}run";

/// A row: its table shared and itself exclusive; a whole table: exclusive; a new row: its table
/// shared. So a row waits for its table and the same row, never another row.
pub(crate) fn locks_of(key: &ClaimKey) -> Vec<Lock> {
    match key {
        ClaimKey::Row { table, key, .. } => vec![(table.clone(), false), (format!("{table}\u{0}{key}"), true)],
        ClaimKey::Table { table, .. } => vec![(table.clone(), true)],
        ClaimKey::NewRow { table, .. } => vec![(table.clone(), false)],
        _ => Vec::new(),
    }
}

/// A lock's name hashed: a Postgres advisory lock's number, a MySQL claims row's key.
pub(crate) fn lock_id(name: &str) -> [u8; 16] {
    let hash = blake3::hash(format!("holon\u{0}{name}").as_bytes());
    hash.as_bytes()[..16].try_into().expect("16 of 32")
}

struct Session {
    conn: Conn,
    /// Each lock it holds, and how many times its holder took it.
    held: BTreeMap<Lock, usize>,
    /// On MySQL: the transaction its record locks are in is open.
    began: bool,
}

impl Session {
    /// Take `lock` without waiting: whether it was free.
    fn take(&mut self, remote: &Remote, (name, exclusive): &Lock) -> Result<bool, String> {
        let id = lock_id(name);
        match &mut self.conn {
            Conn::Pg(p) => {
                let n = i64::from_be_bytes(id[..8].try_into().expect("8 of 16"));
                let sql = if *exclusive { "select pg_try_advisory_lock($1)" } else { "select pg_try_advisory_lock_shared($1)" };
                p.query_one(sql, &[&Text(Some(n.to_string()))]).map(|r| r.get::<_, bool>(0)).map_err(|e| pg_said(&e))
            }
            Conn::My(m) => {
                use mysql::prelude::Queryable;
                let k: String = id.iter().map(|b| format!("{b:02x}")).collect();
                if !self.began {
                    m.query_drop("set transaction isolation level read committed").and_then(|_| m.query_drop("start transaction")).map_err(|e| my_said(&e))?;
                    self.began = true;
                }
                let sql = format!("select k from {CLAIMS_TABLE} where k = ? {}", if *exclusive { "for update nowait" } else { "for share nowait" });
                let mut lock = || match m.exec_first::<String, _, _>(&sql, (&k,)) {
                    Ok(row) => Ok(Some(row.is_some())),
                    Err(mysql::Error::MySqlError(e)) if e.code == 3572 => Ok(None),
                    Err(e) => Err(my_said(&e)),
                };
                match lock()? {
                    Some(true) => return Ok(true),
                    None => return Ok(false),
                    Some(false) => {}
                }
                // No row yet: made on another connection, so this one's transaction holds no lock
                // on it but the one it asks for. Only when there is none: an insert that finds the
                // row another server holds waits for it, past any claim's wait.
                let mut other = remote.conn()?;
                if let Conn::My(o) = other.inner() {
                    // Made by another server in the moment since, and held: waited for a second
                    // at most (a commit sets its own wait on the connection again).
                    let _ = o.query_drop("set session innodb_lock_wait_timeout = 1");
                    match o.exec_drop(format!("insert ignore into {CLAIMS_TABLE} (k) values (?)"), (&k,)) {
                        Ok(()) => {}
                        Err(mysql::Error::MySqlError(e)) if e.code == 1205 => return Ok(false),
                        Err(e) => return Err(my_said(&e)),
                    }
                }
                Ok(lock()?.unwrap_or(false))
            }
        }
    }

    /// Its holder lets go of `lock` once: let go of it in the database when no one holds it.
    /// (A MySQL record lock lasts as long as its transaction: it goes with the session's last.)
    fn drop_one(&mut self, lock: &Lock) {
        let Some(n) = self.held.get_mut(lock) else { return };
        *n -= 1;
        if *n > 0 {
            return;
        }
        self.held.remove(lock);
        if let Conn::Pg(p) = &mut self.conn {
            let id = i64::from_be_bytes(lock_id(&lock.0)[..8].try_into().expect("8 of 16"));
            let sql = if lock.1 { "select pg_advisory_unlock($1)" } else { "select pg_advisory_unlock_shared($1)" };
            let _ = p.query_one(sql, &[&Text(Some(id.to_string()))]);
        }
    }
}

/// A pooled connection, back in the pool when dropped unless it broke.
struct Pooled<'r> {
    remote: &'r Remote,
    conn: Option<Conn>,
    broken: bool,
}

impl Pooled<'_> {
    fn inner(&mut self) -> &mut Conn {
        self.conn.as_mut().expect("a pooled connection until dropped")
    }

    fn into_conn(mut self) -> Conn {
        self.conn.take().expect("a pooled connection until dropped")
    }

    /// Roll back whatever transaction it is in: whether it could.
    fn rollback(&mut self) -> bool {
        match self.inner() {
            Conn::Pg(p) => p.batch_execute("rollback").is_ok(),
            Conn::My(m) => mysql::prelude::Queryable::query_drop(m, "rollback").is_ok(),
        }
    }
}

impl Drop for Pooled<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take()
            && !self.broken
        {
            self.remote.put_back(conn);
        }
    }
}

/// A commit's open transaction on one database: rolled back when dropped uncommitted.
pub(crate) struct Txn<'r> {
    conn: Pooled<'r>,
    open: bool,
}

impl Txn<'_> {
    /// A read as it answers now, under the transaction (its check).
    pub(crate) fn select(&mut self, sql: &str, params: &[Value], g: &mut Gather) -> Result<(), Failure> {
        match self.conn.inner() {
            Conn::Pg(p) => pg_query(p, sql, params, g),
            Conn::My(m) => my_query(m, sql, params, g),
        }
    }

    /// Whether the run `run` is marked done here already.
    pub(crate) fn done(&mut self, run: &str) -> Result<bool, Failure> {
        let sql = format!("select count(*) from {COMMITS_TABLE} where run = $1");
        let mut g = Gather::new(&sql, false, 1, 1 << 20);
        self.select(&sql, &[Value::Text(run.into())], &mut g)?;
        Ok(match g.out.first() {
            Some(Value::Record(r)) => r.values().next().is_some_and(|n| matches!(n, Value::Number(n) if *n > 0.0) || matches!(n, Value::Text(t) if t != "0")),
            _ => false,
        })
    }

    /// Send one statement a run made: the rows it matched.
    pub(crate) fn exec(&mut self, sql: &str, params: &[Value]) -> Result<u64, Failure> {
        match self.conn.inner() {
            Conn::Pg(p) => {
                let stmt = p.prepare(sql).map_err(|e| pg_failed(&e, sql))?;
                let args = pg_args(&stmt, params, sql)?;
                let refs: Vec<&(dyn ToSql + Sync)> = args.iter().map(|a| a as &(dyn ToSql + Sync)).collect();
                p.execute(&stmt, &refs).map_err(|e| pg_failed(&e, sql))
            }
            Conn::My(m) => {
                use mysql::prelude::Queryable;
                let (text, args) = my_args(sql, params)?;
                m.exec_drop(text, args).map_err(|e| my_failed(&e, sql))?;
                Ok(m.affected_rows())
            }
        }
    }

    /// Mark the run `run` done here, in this transaction.
    pub(crate) fn mark(&mut self, run: &str, at: i64) -> Result<(), Failure> {
        self.exec(&format!("insert into {COMMITS_TABLE} (run, at) values ($1, $2)"), &[Value::Text(run.into()), Value::Number(at as f64)]).map(|_| ())
    }

    pub(crate) fn commit(mut self) -> Result<(), Failure> {
        self.open = false;
        let done = match self.conn.inner() {
            Conn::Pg(p) => p.batch_execute("commit").map_err(|e| pg_failed(&e, "commit")),
            Conn::My(m) => mysql::prelude::Queryable::query_drop(m, "commit").map_err(|e| my_failed(&e, "commit")),
        };
        if done.is_err() && !self.conn.rollback() {
            self.conn.broken = true;
        }
        done
    }
}

impl Drop for Txn<'_> {
    fn drop(&mut self) {
        if self.open && !self.conn.rollback() {
            self.conn.broken = true;
        }
    }
}

// ── Postgres ──

/// A parameter in text, which the server parses as the type it inferred for it; none is null.
#[derive(Debug)]
struct Text(Option<String>);

impl ToSql for Text {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match &self.0 {
            Some(t) => {
                out.extend_from_slice(t.as_bytes());
                Ok(IsNull::No)
            }
            None => Ok(IsNull::Yes),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

/// A schema-qualified name with each part quoted, as Postgres reads a name core's parser found.
fn quoted(table: &str) -> String {
    table.split('.').map(|p| format!("\"{}\"", p.replace('"', "\"\""))).collect::<Vec<_>>().join(".")
}

/// A Holon value as text for a parameter the server typed `ty`: true and false as 1 and 0 for a
/// number, a number as `true`/`false` for a boolean, a record or list as JSON.
fn pg_param(v: &Value, ty: &Type) -> Result<Text, String> {
    let number = matches!(*ty, Type::INT2 | Type::INT4 | Type::INT8 | Type::NUMERIC | Type::FLOAT4 | Type::FLOAT8 | Type::OID);
    Ok(Text(Some(match v {
        Value::Number(n) if !n.is_finite() => return Err("a parameter is a finite number".into()),
        Value::Number(n) if *ty == Type::BOOL => (if *n != 0.0 { "true" } else { "false" }).to_string(),
        Value::Number(n) => number_text(*n),
        Value::Text(t) => t.clone(),
        Value::Boolean(b) if number => (if *b { "1" } else { "0" }).to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Sum { arm: 1, .. } => return Ok(Text(None)),
        Value::Sum { payload, .. } => return pg_param(payload, ty),
        Value::List(_) | Value::Record(_) => holon_core::stringify_json(v).ok_or("a parameter's record or list has a value JSON can't hold")?,
        Value::Effect { .. } => return Err("an effect is never a parameter".into()),
    })))
}

/// A number as SQL reads it: a whole one without a point.
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 { (n as i64).to_string() } else { n.to_string() }
}

fn pg_args(stmt: &postgres::Statement, params: &[Value], sql: &str) -> Result<Vec<Text>, Failure> {
    if stmt.params().len() != params.len() {
        return Err(Failure::said(format!("{sql:?} takes {} parameters, not {}", stmt.params().len(), params.len())));
    }
    stmt.params().iter().zip(params).map(|(ty, v)| pg_param(v, ty)).collect::<Result<_, _>>().map_err(Failure::said)
}

fn pg_query(p: &mut postgres::Client, sql: &str, params: &[Value], g: &mut Gather) -> Result<(), Failure> {
    let stmt = p.prepare(sql).map_err(|e| pg_failed(&e, sql))?;
    let args = pg_args(&stmt, params, sql)?;
    let names: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
    let mut rows = p.query_raw(&stmt, args.iter()).map_err(|e| pg_failed(&e, sql))?;
    while let Some(row) = rows.next().map_err(|e| pg_failed(&e, sql))? {
        let mut record = BTreeMap::new();
        for (i, name) in names.iter().enumerate() {
            let Cell(v) = row.try_get(i).map_err(|e| Failure::said(format!("{sql:?}: the column {name:?}: {}", e.into_source().map_or_else(String::new, |s| s.to_string()))))?;
            if let Some(v) = v {
                record.insert(name.clone(), v);
            }
        }
        if !g.take(record).map_err(Failure::said)? {
            break;
        }
    }
    Ok(())
}

fn pg_said(e: &postgres::Error) -> String {
    e.as_db_error().map_or_else(|| e.to_string(), |d| d.message().to_string())
}

fn pg_failed(e: &postgres::Error, sql: &str) -> Failure {
    use postgres::error::SqlState;
    match e.code() {
        Some(c) if *c == SqlState::T_R_SERIALIZATION_FAILURE || *c == SqlState::T_R_DEADLOCK_DETECTED || *c == SqlState::LOCK_NOT_AVAILABLE => {
            Failure { why: format!("{sql:?}: {}", pg_said(e)), again: true }
        }
        Some(c) if *c == SqlState::QUERY_CANCELED => Failure::said(format!("{sql:?} ran past db_statement_ms")),
        _ => Failure::said(format!("{sql:?}: {}", pg_said(e))),
    }
}

/// A column's value as a Holon value; none for null.
struct Cell(Option<Value>);

impl<'a> FromSql<'a> for Cell {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Cell, Box<dyn std::error::Error + Sync + Send>> {
        pg_value(ty, raw).map(|v| Cell(Some(v))).map_err(Into::into)
    }

    fn from_sql_null(_: &Type) -> Result<Cell, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Cell(None))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

fn pg_value(ty: &Type, raw: &[u8]) -> Result<Value, String> {
    let short = || format!("a {ty} value is cut short");
    let int = |n: i64| if n.unsigned_abs() < 1 << 53 { Value::Number(n as f64) } else { Value::Text(n.to_string()) };
    let float = |f: f64| if f.is_finite() { Value::Number(f) } else { Value::Text(f.to_string()) };
    let be = |n: usize| raw.get(..n).ok_or_else(short);
    Ok(match *ty {
        Type::BOOL => Value::Boolean(be(1)?[0] != 0),
        Type::INT2 => int(i16::from_be_bytes(be(2)?.try_into().expect("2")) as i64),
        Type::INT4 => int(i32::from_be_bytes(be(4)?.try_into().expect("4")) as i64),
        Type::INT8 => int(i64::from_be_bytes(be(8)?.try_into().expect("8"))),
        Type::OID => int(u32::from_be_bytes(be(4)?.try_into().expect("4")) as i64),
        Type::FLOAT4 => float(f32::from_be_bytes(be(4)?.try_into().expect("4")) as f64),
        Type::FLOAT8 => float(f64::from_be_bytes(be(8)?.try_into().expect("8"))),
        Type::NUMERIC => Value::Text(numeric(raw).ok_or_else(short)?),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN | Type::CHAR => Value::Text(String::from_utf8_lossy(raw).into_owned()),
        Type::JSON => holon_core::parse_json(&String::from_utf8_lossy(raw)).ok_or("a json value Holon can't read")?,
        Type::JSONB => holon_core::parse_json(&String::from_utf8_lossy(raw.get(1..).ok_or_else(short)?)).ok_or("a jsonb value Holon can't read")?,
        Type::BYTEA => Value::Text(raw.iter().map(|b| format!("{b:02x}")).collect()),
        Type::UUID => {
            let h: String = be(16)?.iter().map(|b| format!("{b:02x}")).collect();
            Value::Text(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
        }
        Type::DATE => {
            let days = i32::from_be_bytes(be(4)?.try_into().expect("4")) as i64;
            Value::Text(day_text(days + PG_EPOCH_DAYS))
        }
        Type::TIMESTAMP | Type::TIMESTAMPTZ => {
            let us = i64::from_be_bytes(be(8)?.try_into().expect("8"));
            let zone = if *ty == Type::TIMESTAMPTZ { "Z" } else { "" };
            Value::Text(match us {
                i64::MAX => "infinity".into(),
                i64::MIN => "-infinity".into(),
                _ => format!("{}{zone}", time_text(us + PG_EPOCH_DAYS * 86_400_000_000)),
            })
        }
        Type::TIME => {
            let us = i64::from_be_bytes(be(8)?.try_into().expect("8"));
            Value::Text(clock_text(us))
        }
        _ if matches!(ty.kind(), Kind::Enum(_)) || ty.name() == "citext" => Value::Text(String::from_utf8_lossy(raw).into_owned()),
        _ => return Err(format!("a {ty} column, which Holon doesn't read: cast it (::text)")),
    })
}

/// Days from 1970-01-01 to 2000-01-01, where Postgres counts dates and times from.
const PG_EPOCH_DAYS: i64 = 10_957;

/// A Postgres numeric (base-10000 digits) as its exact decimal text.
fn numeric(raw: &[u8]) -> Option<String> {
    let be = |i: usize| raw.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let (n, weight, sign, scale) = (be(0)? as usize, be(2)? as i16 as i64, be(4)?, be(6)? as usize);
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let digits: Vec<u16> = (0..n).map(|k| be(8 + 2 * k)).collect::<Option<_>>()?;
    let at = |idx: i64| if idx >= 0 { digits.get(idx as usize).copied().unwrap_or(0) } else { 0 };
    let mut int = String::new();
    for idx in 0..=weight.max(-1) {
        let d = at(idx);
        if int.is_empty() {
            if d != 0 {
                int = d.to_string();
            }
        } else {
            int.push_str(&format!("{d:04}"));
        }
    }
    if int.is_empty() {
        int.push('0');
    }
    let mut frac: String = (0..scale.div_ceil(4) as i64).map(|g| format!("{:04}", at(weight + 1 + g))).collect();
    frac.truncate(scale);
    let negative = sign == 0x4000 && (int != "0" || frac.bytes().any(|b| b != b'0'));
    Some(format!("{}{int}{}{frac}", if negative { "-" } else { "" }, if scale > 0 { "." } else { "" }))
}

/// The date `days` after 1970-01-01, as `YYYY-MM-DD`.
fn day_text(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The time of day `us` microseconds after midnight, as `HH:MM:SS` (with its fraction, if any).
fn clock_text(us: i64) -> String {
    let (s, micro) = (us.div_euclid(1_000_000), us.rem_euclid(1_000_000));
    let fraction = if micro == 0 { String::new() } else { format!(".{micro:06}").trim_end_matches('0').to_string() };
    format!("{:02}:{:02}:{:02}{fraction}", s / 3600, s / 60 % 60, s % 60)
}

/// The moment `us` microseconds after 1970-01-01, as RFC 3339 without its zone.
fn time_text(us: i64) -> String {
    let day = 86_400_000_000;
    format!("{}T{}", day_text(us.div_euclid(day)), clock_text(us.rem_euclid(day)))
}

// ── MySQL ──

fn my_param(v: &Value) -> Result<mysql::Value, String> {
    Ok(match v {
        Value::Number(n) if !n.is_finite() => return Err("a parameter is a finite number".into()),
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => mysql::Value::Int(*n as i64),
        Value::Number(n) => mysql::Value::Double(*n),
        Value::Text(t) => mysql::Value::Bytes(t.clone().into_bytes()),
        Value::Boolean(b) => mysql::Value::Int(i64::from(*b)),
        Value::Sum { arm: 1, .. } => mysql::Value::NULL,
        Value::Sum { payload, .. } => my_param(payload)?,
        Value::List(_) | Value::Record(_) => mysql::Value::Bytes(holon_core::stringify_json(v).ok_or("a parameter's record or list has a value JSON can't hold")?.into_bytes()),
        Value::Effect { .. } => return Err("an effect is never a parameter".into()),
    })
}

/// A statement with `?` for each `$n`, and its parameters in that order.
fn my_args(sql: &str, params: &[Value]) -> Result<(String, Vec<mysql::Value>), Failure> {
    let (text, order) = holon_core::sql::positional(sql).map_err(Failure::said)?;
    let args = order
        .iter()
        .map(|n| params.get(n - 1).ok_or_else(|| format!("{sql:?} uses ${n}, and is given {} parameters", params.len())).and_then(my_param))
        .collect::<Result<_, _>>()
        .map_err(Failure::said)?;
    Ok((text, args))
}

fn my_query(m: &mut mysql::Conn, sql: &str, params: &[Value], g: &mut Gather) -> Result<(), Failure> {
    use mysql::prelude::Queryable;
    let (text, args) = my_args(sql, params)?;
    let rows = m.exec_iter(text, args).map_err(|e| my_failed(&e, sql))?;
    for row in rows {
        let mut row = row.map_err(|e| my_failed(&e, sql))?;
        let columns = row.columns();
        let mut record = BTreeMap::new();
        for (i, column) in columns.iter().enumerate() {
            let name = column.name_str().into_owned();
            let v = row.take::<mysql::Value, _>(i).unwrap_or(mysql::Value::NULL);
            if let Some(v) = my_value(v, column).map_err(|e| Failure::said(format!("{sql:?}: the column {name:?}: {e}")))? {
                record.insert(name, v);
            }
        }
        if !g.take(record).map_err(Failure::said)? {
            break;
        }
    }
    Ok(())
}

fn my_value(v: mysql::Value, column: &mysql::Column) -> Result<Option<Value>, String> {
    use mysql::consts::ColumnType::*;
    let int = |n: i128| if n.unsigned_abs() < 1 << 53 { Value::Number(n as f64) } else { Value::Text(n.to_string()) };
    let ty = column.column_type();
    Ok(Some(match v {
        mysql::Value::NULL => return Ok(None),
        mysql::Value::Int(n) => int(n as i128),
        mysql::Value::UInt(n) => int(n as i128),
        mysql::Value::Float(f) => Value::Number(f as f64),
        mysql::Value::Double(f) => Value::Number(f),
        mysql::Value::Bytes(b) => match ty {
            MYSQL_TYPE_JSON => holon_core::parse_json(&String::from_utf8_lossy(&b)).ok_or("a JSON value Holon can't read")?,
            MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB | MYSQL_TYPE_BLOB | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING | MYSQL_TYPE_VARCHAR | MYSQL_TYPE_BIT
                if column.character_set() == 63 =>
            {
                Value::Text(b.iter().map(|x| format!("{x:02x}")).collect())
            }
            _ => Value::Text(String::from_utf8_lossy(&b).into_owned()),
        },
        mysql::Value::Date(y, mo, d, h, mi, s, us) => {
            let day = format!("{y:04}-{mo:02}-{d:02}");
            match ty {
                MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => Value::Text(day),
                _ => {
                    let us = (h as i64 * 3600 + mi as i64 * 60 + s as i64) * 1_000_000 + us as i64;
                    Value::Text(format!("{day}T{}{}", clock_text(us), if ty == MYSQL_TYPE_TIMESTAMP { "Z" } else { "" }))
                }
            }
        }
        mysql::Value::Time(negative, days, h, mi, s, us) => {
            let us = ((days as i64 * 24 + h as i64) * 3600 + mi as i64 * 60 + s as i64) * 1_000_000 + us as i64;
            Value::Text(format!("{}{}", if negative { "-" } else { "" }, clock_text(us)))
        }
    }))
}

fn my_said(e: &mysql::Error) -> String {
    match e {
        mysql::Error::MySqlError(e) => e.message.clone(),
        e => e.to_string(),
    }
}

fn my_failed(e: &mysql::Error, sql: &str) -> Failure {
    match e {
        // A lock not had in time, a deadlock, a locked row asked for without waiting.
        mysql::Error::MySqlError(m) if matches!(m.code, 1205 | 1213 | 3572) => Failure { why: format!("{sql:?}: {}", m.message), again: true },
        mysql::Error::MySqlError(m) if m.code == 3024 => Failure::said(format!("{sql:?} ran past db_statement_ms")),
        e => Failure::said(format!("{sql:?}: {}", my_said(e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_names_its_database_without_its_credentials() {
        let (engine, at) = address_of("postgres://shop:s3cret@DB.example.com:6543/shop?sslmode=require").unwrap();
        assert_eq!((engine, at.as_str()), (Engine::Postgres, "postgres://db.example.com:6543/shop"));
        let (engine, at) = address_of("mysql://app:s3cret@127.0.0.1/shop?ssl-mode=DISABLED").unwrap();
        assert_eq!((engine, at.as_str()), (Engine::Mysql, "mysql://127.0.0.1:3306/shop"));
        assert!(address_of("mongodb://x").is_err());
        assert!(!address_of("postgres://u:s3cret@h/db").unwrap().1.contains("s3cret"));
        assert_eq!(mysql_tls("mysql://u@db.example.com/shop").unwrap(), ("mysql://u@db.example.com/shop".to_string(), true), "TLS unless it is this machine");
        assert_eq!(mysql_tls("mysql://u@localhost/shop?ssl-mode=required&pool_max=4").unwrap(), ("mysql://u@localhost/shop?pool_max=4".to_string(), true));
    }

    #[test]
    fn exact_decimals_and_times_read_as_text() {
        // 12.50 (scale 2), -0.0042 (weight -1), 10000 (two groups).
        let n = |digits: &[u16], weight: i16, sign: u16, scale: u16| {
            let mut b = Vec::new();
            for x in [digits.len() as u16, weight as u16, sign, scale] {
                b.extend_from_slice(&x.to_be_bytes());
            }
            for d in digits {
                b.extend_from_slice(&d.to_be_bytes());
            }
            numeric(&b).unwrap()
        };
        assert_eq!(n(&[12, 5000], 0, 0, 2), "12.50");
        assert_eq!(n(&[42], -1, 0x4000, 4), "-0.0042");
        assert_eq!(n(&[1], 1, 0, 0), "10000");
        assert_eq!(n(&[], 0, 0, 0), "0");
        assert_eq!(day_text(0), "1970-01-01");
        assert_eq!(day_text(PG_EPOCH_DAYS + 9_430), "2025-10-26");
        assert_eq!(time_text(1_761_480_000_000_000 + 250_000), "2025-10-26T12:00:00.25");
        assert_eq!(clock_text(3_723_000_000), "01:02:03");
    }

    #[test]
    fn a_row_claim_shares_its_table_and_holds_itself() {
        let row = ClaimKey::Row { at: "a".into(), name: "shop".into(), table: "orders".into(), key: "[1]".into() };
        let table = ClaimKey::Table { at: "a".into(), name: "shop".into(), table: "orders".into() };
        assert_eq!(locks_of(&row), [("orders".to_string(), false), ("orders\u{0}[1]".to_string(), true)]);
        assert_eq!(locks_of(&table), [("orders".to_string(), true)]);
        assert_ne!(lock_id("orders"), lock_id("refunds"));
    }
}
