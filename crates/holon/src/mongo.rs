//! The MongoDB databases an admin binds (docs/L4s/databases-L4.md §8, slice 7), Atlas's
//! `mongodb+srv://` addresses included: the official driver's client (its own pool), opened with
//! the connection the admin named (a secret source, fetched by the host and never logged), and
//! what databases.rs asks of them.
//!
//! - **Calls** are the leaves' templates (core's documents.rs) with their holes filled: each `$n`
//!   becomes its value as a BSON value, so a value is data and never an operator: a record given
//!   as a value whose field starts with `$` fails the run. `{ "$oid": … }`, `{ "$date": … }`,
//!   `{ "$numberDecimal": … }` and `{ "$numberLong": … }` in a template make the value that type.
//! - **Values:** whole numbers go as integers (32 bits where they fit), other numbers as doubles,
//!   records as documents, lists as arrays, none as null. Back: numbers as numbers (a long past
//!   2^53 as its digits, as text), an exact decimal as text, an id (`ObjectId`) as its hex, a date
//!   as RFC 3339 text (UTC), bytes as hex; null is a field that isn't there (in a list, none). A
//!   field of another type fails the read, saying to leave it out with a projection.
//! - **Reads** are answered at majority read concern, within `db_statement_ms`.
//! - **A commit** is one transaction (snapshot reads, majority writes): its reads are run again in
//!   it, its writes applied, each `expect` checked against the documents an update matched or a
//!   delete deleted, and the run marked done in `holon_commits`. A document it writes that another
//!   writer changed since it began makes the server end it, and the run goes round again; one it
//!   only reads that a program outside Holon changes in the moment between its check and its
//!   commit isn't seen (Holon's own runs are held off by their claims). A transaction needs a
//!   replica set or a sharded cluster, which Atlas always is: a binding that writes, or is set to
//!   `run`, checks it has one.
//! - **Claims for several servers** (§7.2) are documents in `holon_claims`, one per lock (a
//!   collection shared and a document exclusive, a whole collection exclusive, as engines.rs
//!   takes them on Postgres), each holding its holders with a lease the server's clock keeps:
//!   taken in one atomic update that drops expired holders and adds this one if it fits, renewed
//!   by a thread while held, so a server that stops lets go within [`LEASE`].

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use holon_core::documents::{check, Part, Tpl};
use holon_core::Value;
use mongodb::bson::{doc, oid::ObjectId, Binary, Bson, DateTime, Decimal128, Document};
use mongodb::options::{ClientOptions, CollectionOptions, ReadConcern, ReturnDocument, WriteConcern};
use mongodb::sync::{Client, ClientSession, Collection, Database};
use zeroize::Zeroizing;

use crate::claims::ClaimKey;
use crate::databases::{Gather, COMMITS_TABLE};
use crate::engines::{locks_of, lock_id, Failure, Lock, CLAIMS_TABLE, RUN_LOCK};
use crate::permissions::Permit;

/// How long a claim held in the database lasts unless its server renews it.
pub(crate) const LEASE: Duration = Duration::from_secs(30);

/// One bound MongoDB database: where it is, and the driver's client to it.
pub(crate) struct Mongo {
    /// Where it is, without credentials (`mongodb+srv://cluster0.example.mongodb.net/notes`):
    /// what claims and commit locks know it by.
    pub(crate) address: String,
    client: Client,
    db: Database,
    /// The locks each holder (a run) holds in it, and how many times it took each.
    held: Mutex<BTreeMap<String, BTreeMap<Lock, usize>>>,
    /// The thread renewing those leases, once one is held.
    renewing: Mutex<bool>,
}

impl std::fmt::Debug for Mongo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mongo").field("address", &self.address).finish()
    }
}

/// Whether `url` is a MongoDB connection.
pub(crate) fn is_mongo(url: &str) -> bool {
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("mongodb" | "mongodb+srv"))
}

/// Where `url` points, without its credentials or options: `mongodb://host:port,…/database`.
pub(crate) fn address_of(url: &str) -> Result<String, String> {
    let (scheme, rest) = url.split_once("://").ok_or("a connection is a URL: mongodb://… or mongodb+srv://…")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let hosts = authority.rsplit_once('@').map_or(authority, |(_, h)| h).to_lowercase();
    let database = path.split('?').next().unwrap_or("");
    if hosts.is_empty() {
        return Err("the connection names no host".into());
    }
    if database.is_empty() {
        return Err("the connection names its database: mongodb+srv://…/notes".into());
    }
    Ok(format!("{}://{hosts}/{database}", scheme.to_ascii_lowercase()))
}

impl Mongo {
    /// The database at `url`, reached once now, so a binding that can't connect says so.
    pub(crate) fn open(url: Zeroizing<String>) -> Result<Mongo, String> {
        let address = address_of(&url)?;
        // The driver's own message could repeat the URL: never shown.
        let mut options = ClientOptions::parse(url.as_str()).run().map_err(|_| "the connection isn't a MongoDB URL Holon can read".to_string())?;
        options.app_name = Some("holon".into());
        options.server_selection_timeout = Some(Duration::from_secs(5));
        options.connect_timeout = Some(Duration::from_secs(5));
        let name = options.default_database.clone().ok_or("the connection names its database: mongodb+srv://…/notes")?;
        let client = Client::with_options(options).map_err(|e| format!("{address} can't be reached: {}", said(&e)))?;
        let db = client.database(&name);
        db.run_command(doc! { "ping": 1 }).run().map_err(|e| format!("{address} can't be reached: {}", said(&e)))?;
        Ok(Mongo { address, client, db, held: Mutex::new(BTreeMap::new()), renewing: Mutex::new(false) })
    }

    /// Check it can hold a transaction (a replica set, or a sharded cluster's router), and make
    /// the collections Holon keeps in it: `holon_commits` and `holon_claims`.
    pub(crate) fn keep_collections(&self) -> Result<(), String> {
        let hello = self.db.run_command(doc! { "hello": 1 }).run().map_err(|e| format!("{} didn't answer: {}", self.address, said(&e)))?;
        if hello.get_str("setName").is_err() && hello.get_str("msg").ok() != Some("isdbgrid") {
            return Err(format!(
                "{} is a standalone server: a commit is a transaction, which needs a replica set (Atlas always is one; a server of your own runs with --replSet)",
                self.address
            ));
        }
        for name in [COMMITS_TABLE, CLAIMS_TABLE] {
            match self.db.create_collection(name).run() {
                Ok(()) => {}
                Err(e) if code_of(&e) == Some(48) => {} // there already
                Err(e) => return Err(format!("can't keep {name} in it: {}", said(&e))),
            }
        }
        Ok(())
    }

    /// Its collections, read now, MongoDB's own (`system.…`) left out.
    pub(crate) fn collections(&self) -> Result<Vec<String>, String> {
        let mut names = self.db.list_collection_names().run().map_err(|e| format!("{} didn't list its collections: {}", self.address, said(&e)))?;
        names.retain(|n| !n.starts_with("system."));
        names.sort();
        Ok(names)
    }

    fn collection(&self, name: &str) -> Collection<Document> {
        let options = CollectionOptions::builder().read_concern(ReadConcern::majority()).write_concern(WriteConcern::majority()).build();
        self.db.collection_with_options(name, options)
    }

    /// The documents a find selects, gathered into `g`, within `statement`.
    pub(crate) fn read(&self, f: &Found, g: &mut Gather, statement: Duration) -> Result<(), String> {
        let (filter, sort) = f.filled().map_err(|e| e.why)?;
        let coll = self.collection(f.collection);
        let mut find = coll.find(filter).max_time(statement);
        if let Some(sort) = sort {
            find = find.sort(sort);
        }
        if let Some(n) = f.limit(g) {
            find = find.limit(n);
        }
        let cursor = find.run().map_err(|e| format!("{}: {}", f.what(), said(&e)))?;
        for doc in cursor {
            let doc = doc.map_err(|e| format!("{}: {}", f.what(), said(&e)))?;
            if !g.take(record_of(doc).map_err(|e| format!("{}: {e}", f.what()))?)? {
                break;
            }
        }
        Ok(())
    }

    /// A commit's transaction, each statement `statement` at most.
    pub(crate) fn begin(&self, statement: Duration) -> Result<MongoTxn, Failure> {
        let mut session = self.client.start_session().run().map_err(|e| failed(&e, "begin"))?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .max_commit_time(statement)
            .run()
            .map_err(|e| failed(&e, "begin"))?;
        Ok(MongoTxn { db: self.db.clone(), session, statement, open: true })
    }

    // ── claims for several servers ──

    /// Hold `keys` for `holder` in the database itself, all or none, without waiting: the first
    /// another server's run holds, otherwise. A holder taking a key it holds already takes it again
    /// (and lets go of it as many times).
    pub(crate) fn claim(self: &Arc<Self>, keys: &[ClaimKey], holder: &str) -> Result<(), ClaimKey> {
        let locks: Vec<(usize, Lock)> = keys.iter().enumerate().flat_map(|(i, k)| locks_of(k).into_iter().map(move |l| (i, l))).collect();
        self.hold(&locks, holder).map_err(|i| keys[i].clone())
    }

    /// Hold its run lock (§7.3) for `holder`, shared or `exclusive`, without waiting: whether it
    /// was free.
    pub(crate) fn hold_run(self: &Arc<Self>, holder: &str, exclusive: bool) -> bool {
        self.hold(&[(0, (RUN_LOCK.to_string(), exclusive))], holder).is_ok()
    }

    pub(crate) fn let_go_run(&self, holder: &str, exclusive: bool) {
        self.let_go(&[(RUN_LOCK.to_string(), exclusive)], holder);
    }

    pub(crate) fn unclaim(&self, keys: &[ClaimKey], holder: &str) {
        let locks: Vec<Lock> = keys.iter().flat_map(locks_of).collect();
        self.let_go(&locks, holder);
    }

    fn hold(self: &Arc<Self>, locks: &[(usize, Lock)], holder: &str) -> Result<(), usize> {
        let mut taken: Vec<&Lock> = Vec::new();
        for (key, lock) in locks {
            let has = self.held.lock().unwrap().get(holder).and_then(|h| h.get(lock)).copied().unwrap_or(0);
            if has == 0 {
                match self.take(lock, holder) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => {
                        for l in &taken {
                            self.drop_one(l, holder);
                        }
                        return Err(*key);
                    }
                }
            }
            *self.held.lock().unwrap().entry(holder.to_string()).or_default().entry(lock.clone()).or_insert(0) += 1;
            taken.push(lock);
        }
        if !locks.is_empty() {
            self.keep_renewing();
        }
        Ok(())
    }

    fn let_go(&self, locks: &[Lock], holder: &str) {
        for lock in locks {
            self.drop_one(lock, holder);
        }
    }

    /// Its holder lets go of `lock` once: let go of it in the database when it holds it no more.
    fn drop_one(&self, lock: &Lock, holder: &str) {
        let mut held = self.held.lock().unwrap();
        let Some(h) = held.get_mut(holder) else { return };
        let Some(n) = h.get_mut(lock) else { return };
        *n -= 1;
        if *n > 0 {
            return;
        }
        h.remove(lock);
        if h.is_empty() {
            held.remove(holder);
        }
        drop(held);
        let claims = self.collection(CLAIMS_TABLE);
        let id = lock_hex(&lock.0);
        let _ = claims.update_one(doc! { "_id": &id }, doc! { "$pull": { "h": { "k": holder } } }).run();
        let _ = claims.delete_one(doc! { "_id": &id, "h": { "$size": 0 } }).run();
    }

    /// Take `lock` for `holder` without waiting, in one atomic update of its document: holders
    /// whose lease ran out are dropped, and this one added if it fits (exclusive: no other holder;
    /// shared: no exclusive one). Whether it was taken.
    fn take(&self, (name, exclusive): &Lock, holder: &str) -> Result<bool, String> {
        let lease = LEASE.as_millis() as i64;
        let fits = if *exclusive {
            doc! { "$eq": [{ "$size": "$h" }, 0] }
        } else {
            doc! { "$not": [{ "$anyElementTrue": [{ "$map": { "input": "$h", "in": "$$this.x" } }] }] }
        };
        let pipeline = vec![
            doc! { "$set": { "h": { "$filter": {
                "input": { "$ifNull": ["$h", []] },
                "cond": { "$and": [{ "$gt": ["$$this.u", "$$NOW"] }, { "$ne": ["$$this.k", holder] }] },
            } } } },
            doc! { "$set": { "h": { "$cond": [fits, { "$concatArrays": ["$h", [{ "k": holder, "x": *exclusive, "u": { "$add": ["$$NOW", lease] } }]] }, "$h"] } } },
        ];
        let claims = self.collection(CLAIMS_TABLE);
        let id = lock_hex(name);
        // Two servers making the same lock's document at once: one of them finds it made.
        for _ in 0..2 {
            match claims.find_one_and_update(doc! { "_id": &id }, pipeline.clone()).upsert(true).return_document(ReturnDocument::After).run() {
                Ok(doc) => {
                    let holders = doc.as_ref().and_then(|d| d.get_array("h").ok()).cloned().unwrap_or_default();
                    return Ok(holders.iter().any(|h| matches!(h, Bson::Document(d) if d.get_str("k").ok() == Some(holder))));
                }
                Err(e) if code_of(&e) == Some(11000) => continue,
                Err(e) => {
                    eprintln!("holon: can't claim in {}: {}", self.address, said(&e));
                    return Err(said(&e));
                }
            }
        }
        Ok(false)
    }

    /// Renew every lease held here, every third of [`LEASE`], for as long as any is held and the
    /// database stays bound.
    fn keep_renewing(self: &Arc<Self>) {
        let mut renewing = self.renewing.lock().unwrap();
        if *renewing {
            return;
        }
        *renewing = true;
        let me: Weak<Mongo> = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(LEASE / 3);
                let Some(m) = me.upgrade() else { return };
                let holders: Vec<String> = m.held.lock().unwrap().keys().cloned().collect();
                if holders.is_empty() {
                    let mut renewing = m.renewing.lock().unwrap();
                    if m.held.lock().unwrap().is_empty() {
                        *renewing = false;
                        return;
                    }
                    continue;
                }
                let lease = LEASE.as_millis() as i64;
                let renew = vec![doc! { "$set": { "h": { "$map": { "input": "$h", "in": { "$cond": [
                    { "$in": ["$$this.k", &holders] },
                    { "$mergeObjects": ["$$this", { "u": { "$add": ["$$NOW", lease] } }] },
                    "$$this",
                ] } } } } }];
                if let Err(e) = m.collection(CLAIMS_TABLE).update_many(doc! { "h.k": { "$in": &holders } }, renew).run() {
                    eprintln!("holon: can't renew claims in {}: {}", m.address, said(&e));
                }
            }
        });
    }
}

/// A lock's name hashed, as its document's id.
fn lock_hex(name: &str) -> String {
    lock_id(name).iter().map(|b| format!("{b:02x}")).collect()
}

// ── what a run asks ──

/// A find a run asked (`{database, find, filter, params, one, sort?, limit?}`), its templates read.
pub(crate) struct Found<'r> {
    pub(crate) database: &'r str,
    pub(crate) collection: &'r str,
    pub(crate) filter: Tpl,
    sort: Option<Tpl>,
    limit: Option<i64>,
    pub(crate) params: &'r [Value],
    pub(crate) one: bool,
}

impl Found<'_> {
    /// What it is, for a message: `find on "notes"`.
    pub(crate) fn what(&self) -> String {
        format!("find on {:?}", self.collection)
    }

    fn filled(&self) -> Result<(Document, Option<Document>), Failure> {
        let filter = document(&self.filter, self.params)?;
        let sort = self.sort.as_ref().map(|s| document(s, &[])).transpose()?;
        Ok((filter, sort))
    }

    /// How many to ask the server for: one for `one`, else its limit; and never more than one
    /// past what the run may still read, so a read past it fails without reading them all.
    fn limit(&self, g: &Gather) -> Option<i64> {
        if self.one {
            return Some(1);
        }
        let room = i64::try_from(g.rows_left().saturating_add(1)).unwrap_or(i64::MAX);
        Some(self.limit.map_or(room, |l| l.min(room)))
    }
}

/// Read a find request.
pub(crate) fn found(request: &Value) -> Result<Found<'_>, String> {
    let Value::Record(r) = request else { return Err("a database read is a record".into()) };
    let (Some(Value::Text(database)), Some(Value::Text(collection)), Some(Value::Text(filter)), Some(Value::List(params))) = (r.get("database"), r.get("find"), r.get("filter"), r.get("params")) else {
        return Err("a find names its database, collection, filter and params".into());
    };
    let filter = check(Part::Filter, filter)?.0;
    let sort = match r.get("sort") {
        Some(Value::Text(s)) => Some(check(Part::Sort, s)?.0),
        _ => None,
    };
    let limit = match r.get("limit") {
        Some(Value::Number(n)) if *n >= 1.0 => Some(*n as i64),
        _ => None,
    };
    Ok(Found { database, collection, filter, sort, limit, params, one: matches!(r.get("one"), Some(Value::Boolean(true))) })
}

/// A write a run made, its templates read and checked against its grant again.
pub(crate) struct Write<'e> {
    pub(crate) verb: &'static str,
    pub(crate) collection: &'e str,
    /// The filter of an update or a delete; the document of an insert.
    pub(crate) first: Tpl,
    /// An update's change.
    change: Option<Tpl>,
    pub(crate) params: &'e [Value],
    pub(crate) expect: Option<f64>,
}

/// One write a run made (`{insert|update|delete: <collection>, …}`) on the database `name`.
pub(crate) fn write_of<'e>(name: &str, exec: &'e Value, permit: &Permit) -> Result<Write<'e>, String> {
    use crate::databases::WRITE_FAILED;
    let Value::Record(e) = exec else { return Err(format!("{WRITE_FAILED}a write is a record")) };
    let Some(Value::List(params)) = e.get("params") else { return Err(format!("{WRITE_FAILED}a write has its params")) };
    let text = |k: &str| match e.get(k) {
        Some(Value::Text(t)) => Ok(t.as_str()),
        _ => Err(format!("{WRITE_FAILED}a write has its {k}")),
    };
    let (verb, collection) = match ["insert", "update", "delete"].into_iter().find_map(|v| match e.get(v) {
        Some(Value::Text(c)) => Some((v, c.as_str())),
        _ => None,
    }) {
        Some(found) => found,
        None => return Err(format!("{WRITE_FAILED}a write names its collection")),
    };
    let at = format!("{name}/{collection}");
    let mut needs = vec![holon_core::Capability::DbWrite(at.clone())];
    if verb != "insert" {
        needs.push(holon_core::Capability::DbRead(at));
    }
    if let Some(c) = needs.iter().find(|c| !permit.allows(c)) {
        return Err(format!("{WRITE_FAILED}a write on {collection:?} needs {c:?}, which isn't granted to this deployment"));
    }
    let checked = |part: Part, t: &str| check(part, t).map(|(t, _)| t).map_err(|why| format!("{WRITE_FAILED}{verb} on {collection:?}: {why}"));
    let (first, change) = match verb {
        "insert" => (checked(Part::Document, text("document")?)?, None),
        "update" => (checked(Part::Filter, text("filter")?)?, Some(checked(Part::Update, text("change")?)?)),
        _ => (checked(Part::Filter, text("filter")?)?, None),
    };
    let expect = match e.get("expect") {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    };
    Ok(Write { verb, collection, first, change, params, expect })
}

// ── what a call claims ──

/// What a call says about the one document it is about, if it is: a filter's `_id`, or an
/// insert's document's.
pub(crate) struct Pins {
    tpl: Tpl,
    insert: bool,
}

impl Pins {
    pub(crate) fn filter(t: &Tpl) -> Pins {
        Pins { tpl: t.clone(), insert: false }
    }

    /// The `_id` it gives, as written.
    pub(crate) fn id(&self) -> Option<&Tpl> {
        if self.insert { self.tpl.get("_id") } else { holon_core::documents::pinned_id(&self.tpl) }
    }

    pub(crate) fn is_insert(&self) -> bool {
        self.insert
    }
}

/// A write's collection, what it pins, and its params, if `e` is a document database's write.
pub(crate) fn write_shape(e: &BTreeMap<String, Value>) -> Option<(&str, Pins, &[Value])> {
    let Some(Value::List(params)) = e.get("params") else { return None };
    let text = |k: &str| match e.get(k) {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    if let Some(c) = text("insert") {
        let tpl = holon_core::documents::parse(text("document")?).ok()?;
        return Some((c, Pins { tpl, insert: true }, params));
    }
    let c = text("update").or_else(|| text("delete"))?;
    let tpl = holon_core::documents::parse(text("filter")?).ok()?;
    Some((c, Pins { tpl, insert: false }, params))
}

/// An `_id` as one value a claim can key by: a constant, a hole's value (a text, number or
/// true/false), or either inside `{ "$oid": … }`.
pub(crate) fn scalar_of(t: &Tpl, params: &[Value]) -> Option<Value> {
    match t {
        Tpl::Text(s) => Some(Value::Text(s.clone())),
        Tpl::Number(n) => n.parse().ok().map(Value::Number),
        Tpl::Bool(b) => Some(Value::Boolean(*b)),
        Tpl::Hole(n) => params.get(n - 1).filter(|v| matches!(v, Value::Text(_) | Value::Number(_) | Value::Boolean(_))).cloned(),
        Tpl::Object(fields) if fields.len() == 1 && fields[0].0 == "$oid" => scalar_of(&fields[0].1, params),
        _ => None,
    }
}

// ── the commit's transaction ──

/// A commit's open transaction on one MongoDB database: aborted when dropped uncommitted.
pub(crate) struct MongoTxn {
    db: Database,
    session: ClientSession,
    statement: Duration,
    open: bool,
}

impl MongoTxn {
    fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }

    /// A find as it answers now, in the transaction (its check).
    pub(crate) fn find(&mut self, f: &Found, g: &mut Gather) -> Result<(), Failure> {
        let (filter, sort) = f.filled()?;
        let coll = self.collection(f.collection);
        let mut find = coll.find(filter).max_time(self.statement);
        if let Some(sort) = sort {
            find = find.sort(sort);
        }
        if let Some(n) = f.limit(g) {
            find = find.limit(n);
        }
        let mut cursor = find.session(&mut self.session).run().map_err(|e| failed(&e, &f.what()))?;
        while let Some(doc) = cursor.next(&mut self.session) {
            let doc = doc.map_err(|e| failed(&e, &f.what()))?;
            let record = record_of(doc).map_err(|why| Failure { why, again: false })?;
            match g.take(record) {
                Ok(true) => {}
                Ok(false) => break,
                Err(why) => return Err(Failure { why, again: false }),
            }
        }
        Ok(())
    }

    /// Whether the run `run` is marked done here already.
    pub(crate) fn done(&mut self, run: &str) -> Result<bool, Failure> {
        let found = self.collection(COMMITS_TABLE).find_one(doc! { "_id": run }).session(&mut self.session).run().map_err(|e| failed(&e, COMMITS_TABLE))?;
        Ok(found.is_some())
    }

    /// Apply one write a run made: the documents an update matched, a delete deleted, an insert
    /// added.
    pub(crate) fn write(&mut self, w: &Write) -> Result<u64, Failure> {
        let what = format!("{} on {:?}", w.verb, w.collection);
        let first = document(&w.first, w.params)?;
        let coll = self.collection(w.collection);
        match (w.verb, &w.change) {
            ("insert", _) => coll.insert_one(first).session(&mut self.session).run().map(|_| 1).map_err(|e| match code_of(&e) {
                // The data said no, as an unmet `expect` does: it isn't run again.
                Some(11000) => Failure { why: format!("{}{what}: a document with its _id is there already", crate::databases::UNMET), again: false },
                _ => failed(&e, &what),
            }),
            ("update", Some(change)) => {
                let change = document(change, w.params)?;
                coll.update_many(first, change).session(&mut self.session).run().map(|r| r.matched_count).map_err(|e| failed(&e, &what))
            }
            _ => coll.delete_many(first).session(&mut self.session).run().map(|r| r.deleted_count).map_err(|e| failed(&e, &what)),
        }
    }

    /// Mark the run `run` done here, in this transaction.
    pub(crate) fn mark(&mut self, run: &str, at: i64) -> Result<(), Failure> {
        self.collection(COMMITS_TABLE)
            .insert_one(doc! { "_id": run, "at": DateTime::from_millis(at) })
            .session(&mut self.session)
            .run()
            .map(|_| ())
            .map_err(|e| failed(&e, COMMITS_TABLE))
    }

    pub(crate) fn commit(mut self) -> Result<(), Failure> {
        self.open = false;
        self.session.commit_transaction().run().map_err(|e| failed(&e, "commit"))
    }
}

impl Drop for MongoTxn {
    fn drop(&mut self) {
        if self.open {
            let _ = self.session.abort_transaction().run();
        }
    }
}

/// Why a call failed; `again` when it was two transactions meeting (a write conflict, a
/// transaction the server ended, a commit whose result is unknown), so the run goes round again.
fn failed(e: &mongodb::error::Error, what: &str) -> Failure {
    use mongodb::error::{TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT};
    let again = e.contains_label(TRANSIENT_TRANSACTION_ERROR) || e.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) || matches!(code_of(e), Some(112 | 24 | 251));
    Failure { why: format!("{what}: {}", said(e)), again }
}

/// The server's code for an error, if it has one.
fn code_of(e: &mongodb::error::Error) -> Option<i32> {
    use mongodb::error::{ErrorKind, WriteFailure};
    match e.kind.as_ref() {
        ErrorKind::Command(c) => Some(c.code),
        ErrorKind::Write(WriteFailure::WriteError(w)) => Some(w.code),
        ErrorKind::Write(WriteFailure::WriteConcernError(w)) => Some(w.code),
        ErrorKind::InsertMany(i) => i.write_errors.as_ref().and_then(|w| w.first()).map(|w| w.code),
        _ => None,
    }
}

/// An error said for the log: the server's message, or the driver's.
fn said(e: &mongodb::error::Error) -> String {
    use mongodb::error::{ErrorKind, WriteFailure};
    match e.kind.as_ref() {
        ErrorKind::Command(c) => format!("{} ({})", c.message, c.code_name),
        ErrorKind::Write(WriteFailure::WriteError(w)) => w.message.clone(),
        ErrorKind::Write(WriteFailure::WriteConcernError(w)) => w.message.clone(),
        _ => e.kind.to_string(),
    }
}

// ── values ──

/// A template with its holes filled by `params`, as a document.
fn document(t: &Tpl, params: &[Value]) -> Result<Document, Failure> {
    match bson_of(t, params) {
        Ok(Bson::Document(d)) => Ok(d),
        Ok(_) => Err(Failure { why: "a filter, update or document is a record".into(), again: false }),
        Err(why) => Err(Failure { why, again: false }),
    }
}

fn bson_of(t: &Tpl, params: &[Value]) -> Result<Bson, String> {
    Ok(match t {
        Tpl::Null => Bson::Null,
        Tpl::Bool(b) => Bson::Boolean(*b),
        Tpl::Number(n) => match n.parse::<i64>() {
            Ok(i) => integer(i),
            Err(_) => Bson::Double(n.parse().map_err(|_| format!("{n:?} isn't a number"))?),
        },
        Tpl::Text(s) => Bson::String(s.clone()),
        Tpl::Hole(n) => {
            let v = params.get(n - 1).ok_or_else(|| format!("the call has ${n} but {} values", params.len()))?;
            value_bson(v, 0)?
        }
        Tpl::List(items) => Bson::Array(items.iter().map(|i| bson_of(i, params)).collect::<Result<_, _>>()?),
        Tpl::Object(fields) if fields.len() == 1 && typed(&fields[0].0) => {
            let (kind, inner) = &fields[0];
            as_typed(kind, bson_of(inner, params)?)?
        }
        Tpl::Object(fields) => {
            let mut d = Document::new();
            for (k, v) in fields {
                d.insert(k.clone(), bson_of(v, params)?);
            }
            Bson::Document(d)
        }
    })
}

/// The extended-JSON wrappers a template may give a value: what type to make it.
fn typed(key: &str) -> bool {
    matches!(key, "$oid" | "$date" | "$numberDecimal" | "$numberLong")
}

fn as_typed(kind: &str, v: Bson) -> Result<Bson, String> {
    let text = match &v {
        Bson::String(s) => s.clone(),
        Bson::Int32(n) => n.to_string(),
        Bson::Int64(n) => n.to_string(),
        Bson::Double(f) => f.to_string(),
        other => return Err(format!("{kind} takes a text or a number, not {other}")),
    };
    Ok(match kind {
        "$oid" => Bson::ObjectId(ObjectId::parse_str(text.trim()).map_err(|_| format!("{text:?} isn't an id: 24 hex digits"))?),
        "$date" => match &v {
            Bson::String(s) => Bson::DateTime(DateTime::parse_rfc3339_str(s.trim()).map_err(|_| format!("{s:?} isn't an RFC 3339 time"))?),
            _ => Bson::DateTime(DateTime::from_millis(text.parse::<f64>().map_err(|_| format!("{text:?} isn't a time"))? as i64)),
        },
        "$numberDecimal" => Bson::Decimal128(Decimal128::from_str(text.trim()).map_err(|_| format!("{text:?} isn't a decimal"))?),
        _ => Bson::Int64(text.trim().parse().map_err(|_| format!("{text:?} isn't a whole number"))?),
    })
}

fn integer(i: i64) -> Bson {
    i32::try_from(i).map_or(Bson::Int64(i), Bson::Int32)
}

/// A Holon value as a BSON value: data only, never an operator.
fn value_bson(v: &Value, depth: usize) -> Result<Bson, String> {
    if depth > 64 {
        return Err("a value nests more than 64 deep".into());
    }
    Ok(match v {
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => integer(*n as i64),
        Value::Number(n) if n.is_finite() => Bson::Double(*n),
        Value::Number(_) => return Err("a value is a finite number".into()),
        Value::Text(t) => Bson::String(t.clone()),
        Value::Boolean(b) => Bson::Boolean(*b),
        Value::Sum { arm: 1, .. } => Bson::Null,
        Value::Sum { payload, .. } => value_bson(payload, depth + 1)?,
        Value::List(items) => Bson::Array(items.iter().map(|i| value_bson(i, depth + 1)).collect::<Result<_, _>>()?),
        Value::Record(fields) => {
            let mut d = Document::new();
            for (k, v) in fields {
                if k.starts_with('$') {
                    return Err(format!("a value is data: its field {k:?} starts with $, which MongoDB would read as an operator"));
                }
                d.insert(k.clone(), value_bson(v, depth + 1)?);
            }
            Bson::Document(d)
        }
        Value::Effect { .. } => return Err("an effect is never a value".into()),
    })
}

/// A document as a record: each field as [`value_of`] reads it, null fields left out.
fn record_of(doc: Document) -> Result<BTreeMap<String, Value>, String> {
    let mut record = BTreeMap::new();
    for (k, v) in doc {
        if let Some(v) = value_of(&k, v)? {
            record.insert(k, v);
        }
    }
    Ok(record)
}

fn none() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}

/// A BSON value as a Holon value: `None` for null (the field isn't there).
fn value_of(field: &str, v: Bson) -> Result<Option<Value>, String> {
    Ok(Some(match v {
        Bson::Null | Bson::Undefined => return Ok(None),
        Bson::Double(f) => Value::Number(f),
        Bson::Int32(n) => Value::Number(f64::from(n)),
        Bson::Int64(n) if n.unsigned_abs() < 1 << 53 => Value::Number(n as f64),
        Bson::Int64(n) => Value::Text(n.to_string()),
        Bson::String(s) => Value::Text(s),
        Bson::Boolean(b) => Value::Boolean(b),
        Bson::Decimal128(d) => Value::Text(d.to_string()),
        Bson::ObjectId(id) => Value::Text(id.to_hex()),
        Bson::DateTime(t) => Value::Text(t.try_to_rfc3339_string().map_err(|_| format!("the field {field:?} is a date past what RFC 3339 writes"))?),
        Bson::Binary(Binary { bytes, .. }) => Value::Text(bytes.iter().map(|b| format!("{b:02x}")).collect()),
        Bson::Array(items) => Value::List(items.into_iter().map(|i| value_of(field, i).map(|v| v.unwrap_or_else(none))).collect::<Result<_, _>>()?),
        Bson::Document(d) => Value::Record(record_of(d)?),
        other => return Err(format!("the field {field:?} is a {:?}, which Holon doesn't read: leave it out of what the find selects", other.element_type())),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: f64) -> Value {
        Value::Number(v)
    }

    #[test]
    fn a_connection_names_its_database_without_its_credentials() {
        assert_eq!(address_of("mongodb+srv://ada:s3cr%2Ft@Cluster0.ABC.mongodb.net/notes?retryWrites=true").unwrap(), "mongodb+srv://cluster0.abc.mongodb.net/notes");
        assert_eq!(address_of("mongodb://127.0.0.1:27017,127.0.0.1:27018/app?replicaSet=rs0").unwrap(), "mongodb://127.0.0.1:27017,127.0.0.1:27018/app");
        assert!(address_of("mongodb://127.0.0.1:27017/?replicaSet=rs0").unwrap_err().contains("names its database"));
        assert!(is_mongo("mongodb+srv://x/y") && is_mongo("MongoDB://x/y") && !is_mongo("postgres://x/y"));
    }

    #[test]
    fn holes_take_values_as_data_and_wrappers_make_their_type() {
        let t = check(Part::Filter, r#"{"_id":{"$oid":$1},"n":{"$gt":$2},"at":{"$date":$3},"tags":["a",$4]}"#).unwrap().0;
        let id = "65a1b2c3d4e5f60718293a4b";
        let params = [Value::Text(id.into()), n(5.0), Value::Text("2026-10-26T12:00:00Z".into()), n(2.5)];
        let d = document(&t, &params).unwrap();
        assert_eq!(d.get_object_id("_id").unwrap().to_hex(), id);
        assert_eq!(d.get_document("n").unwrap().get_i32("$gt").unwrap(), 5);
        assert!(matches!(d.get("at"), Some(Bson::DateTime(_))));
        assert_eq!(d.get_array("tags").unwrap()[1], Bson::Double(2.5));
        // A record given as a value can never be an operator.
        let sneaky = Value::Record(BTreeMap::from([("$ne".to_string(), Value::Text("x".into()))]));
        let filter = check(Part::Filter, r#"{"owner":$1}"#).unwrap().0;
        assert!(document(&filter, &[sneaky]).unwrap_err().why.contains("starts with $"));
        let nested = Value::Record(BTreeMap::from([("a".to_string(), Value::List(vec![Value::Record(BTreeMap::from([("$where".to_string(), n(1.0))]))]))]));
        assert!(document(&filter, &[nested]).is_err(), "at any depth");
        assert!(document(&filter, &[]).unwrap_err().why.contains("$1 but 0 values"));
    }

    #[test]
    fn documents_read_as_records_their_nulls_left_out() {
        let id = ObjectId::new();
        let doc = doc! { "_id": id, "n": 3_i32, "big": 9_007_199_254_740_993_i64, "price": Decimal128::from_str("12.50").unwrap(), "gone": Bson::Null, "tags": ["a", Bson::Null], "at": DateTime::from_millis(0), "inner": { "x": 1.5 } };
        let r = record_of(doc).unwrap();
        assert_eq!(r["_id"], Value::Text(id.to_hex()));
        assert_eq!((r["n"].clone(), r["big"].clone(), r["price"].clone()), (n(3.0), Value::Text("9007199254740993".into()), Value::Text("12.50".into())));
        assert!(!r.contains_key("gone"));
        assert_eq!(r["tags"], Value::List(vec![Value::Text("a".into()), none()]));
        assert_eq!(r["at"], Value::Text("1970-01-01T00:00:00Z".into()));
        assert_eq!(r["inner"], Value::Record(BTreeMap::from([("x".to_string(), n(1.5))])));
        assert!(record_of(doc! { "min": Bson::MinKey }).unwrap_err().contains("leave it out"));
    }
}
