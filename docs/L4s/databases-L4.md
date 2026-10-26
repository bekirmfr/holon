# Databases: SQL and documents as host resources, and how runs share what they write (L4)

**Status:** slices 1 to 4 built (2026-10-26): commit locks per file, with intent locks on the
folder (`locks.rs`, §7.1); SQLite databases (`databases.rs`, the `database` header,
`db_query`/`db_one`/`db_exec`, `DbLimits`, the commit transaction with `holon_commits`; sample
`orders-api`, guide `docs/manual/guides/databases.md`); and claims on rows, tables, files and
folders in the server's table (`claims.rs`, §7.2; sample `refund-once`); Postgres and MySQL
(`engines.rs`, §8), with claims held in the database for several servers. A database's reads are
logged whole: its `keep` (§5) isn't built. Slices 5 to 7 are design. Written 2026-10-25, against `main` at 6ee0c58. Builds on
[host resources](host-resources-L4.md) (also design only: resources the host defines, their
addresses and timeline) and [key claims](key-claims-L4.md) (built: `claims.rs`).

A flow keeps its own data in its state and its collections: a deployment's, key by key, written
when the run finishes. That is what makes a run atomic, replayable and checkable offline, and it
stays the store a flow has without asking. But a business already has its data somewhere: a
Postgres the shop runs on, a SQLite file beside an app, a MongoDB Atlas cluster. This note makes
a **database a host resource**, as a named folder is: an admin defines it, a flow names it, the
manifest says which **tables** it reads and writes, a query's rows are the run's input (logged,
so replay needs no database), and a run's writes land **in one transaction when it finishes**.

```holon
flow refunds
port 8110
database shop

route POST /refunds/:order (req) {
  order = shop.one("select id, total from orders where id = $1 and not refunded", req.params.order)
  paid = http.post("httpbin.org", "/anything/refunds", body: { order: order.id, amount: order.total })
  shop.exec("update orders set refunded = true where id = $1 and not refunded", order.id, expect: 1)
  shop.exec("insert into refunds (order_id, amount) values ($1, $2)", order.id, order.total)
  respond { refunded: order.total, payment: paid.status }
}
```

`holon check` would say, in the shape it has today (`commands.rs`: `it may:` from `describe.rs`,
`while it runs:` from `while_it_runs`):

```
$ holon check refunds.holon
…
a server on :8110
  POST /refunds/:order
unit 9c41…
it may:
  read table "orders" of database "shop"
  write table "orders" of database "shop"
  write table "refunds" of database "shop"
  call https://httpbin.org
  receive requests
while it runs:
  holds the rows it reads in "shop/orders" from its first outbound call until it commits, so two runs on one row call out once
```

It also settles how runs share what they write (§7). Runs stay optimistic, as they are: each
runs on what it read, and its commit checks that what it read is unchanged. That check and the
writes happen under a **commit lock** on each shared resource, held for milliseconds. A run about
to call out **claims** the rows it read in tables it may write. Locking a resource for a **whole
run** is a setting an admin turns on for a resource that needs it, not the default.

---

## 1. What is already there

Each point names where it is.

**Stores.**

- **State and collections are a deployment's.** A collection's root is a state cell under
  `#collection:<name>`, its items in the content store (`collections.rs`, module docs,
  `cell_key`). Read by key or listed by prefix (`kind.rs`, `capabilities_of`: `collection_get`,
  `collection_list`); no query, no index, no sharing between deployments.
- **Named folders are the host's.** An admin binds a name to a path, for the server or one
  deployment (`folders.rs`, `Bound { server, deployments }`), with an access (`FolderAccess`:
  `Read`, `ReadWrite`), what reads keep (`Keep`: `bytes`, `bytes for 30d`, `hash`) and an
  optional total `cap`.
- **Writes are whole values.** A state write sets a key, a put sets an item, a file write puts
  the whole file (`firing.rs`, `commit`). Applying one again leaves the same result, which §6
  relies on and SQL doesn't give (`n = n - 1`).

**What a flow may do.**

- **Capabilities come from frozen parameters,** one match arm per leaf (`kind.rs`,
  `capabilities_of`): "the collection is a frozen param, so the capability names it exactly"; an
  `http.*` host likewise (`http_host(params)`), which is why the language requires the host
  written as text.
- **A capability is one name** (`FileRead(String)`, `HttpRequest(String)`, …, `kind.rs`,
  `Capability`). The manifest and grant hold them as their debug text (`FileRead("inbox")`): the
  permit checks by it (`permissions.rs`, `Permit::allows`), lists one kind's names by it
  (`Permit::named`), the folders refuse a deploy by it (`folders.rs`, `Folders::refuses`: an
  unbound folder, or a write to one bound read-only), and `describe.rs` says each kind in words.
- **The policy** (`holon.toml`, `permissions.rs`, `Policy`) sets each limit's default and ceiling,
  grants without asking (`auto`: an item, or a whole kind as `StateRead(*)`), and refuses only
  plain HTTP and private addresses unless allowed (`refuses_item`).
- **Limits are per run and granted:** `Limits` (`holon-host/src/rounds.rs`: run budget 25 s for a
  request and 60 s for a clock run, `claim_wait` 10 s, …) and `FileLimits` (`permissions.rs`:
  bytes read and written per run, files per run), each with defaults and ceilings.

**Runs.**

- **A deployment that touches state, collections or folders has a state cell** (`deploy.rs`,
  `derives_state`: "A folder's reads are checked, and its writes committed, as a collection's
  are."). One that doesn't runs with no commit step (`firing.rs`, `fire_with_state`: `let
  Some(cell) = state else { return … }`).
- **Runs are optimistic** (`fire_with_state`). Each attempt runs on a copy of the state, no lock
  held, so runs of one deployment proceed side by side. Then, holding the deployment's state lock
  (`cell.state.lock()`), it checks that every state cell, collection read and file read is still
  as it found it (`still_as_read`) and applies its writes (`commit`). Otherwise it runs again from
  the state as it now is, its outbound requests answered from its own record (`prior`), not sent
  again. After `OPTIMISTIC_ATTEMPTS` (4) it runs once more holding the lock.
- **A run's reads go through its `RunWaits`** (`durable.rs`): `collections` and `files` answer
  them as the run found them, `reads` keeps each once with its answer (`collections::Read`, with
  a file's hash, size and kept text in `file: Option<folders::Seen>`), and `failed` is a read that
  fails the run (a file too big: `folders.rs`, `READ_FAILED`, answered as a 500).
- **The log entry keeps them** (`recorder.rs`, `LogEntry`): `reads` (`LoggedRead`, a file's as
  `LoggedFile`: hash, size, the blob its text is in while the folder keeps it), `retries`,
  `claims` (`LoggedClaim`), `rounds`.

**Claims** (`claims.rs`, the key-claims L4).

- **What's claimed:** the keys a run **read** in collections the grant lets it **write**
  (`Claiming::keys_of`), item by item, or a whole collection for a listing (`ClaimKey::Item`,
  `ClaimKey::Collection`), never a state cell. Taken before the run's requests go out, re-checking
  those reads as they're taken (`Claiming::claim`). Files aren't claimed.
- **How long:** held until the run commits, is discarded or parks (module docs). A run never
  waits while holding (it releases and runs again), so there's no deadlock; waits are bounded by
  `claim_wait`. Past it, a request is turned away as busy, a 503 naming the key (`tests.rs`,
  `busy_after_claim_wait`).
- **Where they live:** the table is in the deployment's `StateCell` (`claims.rs`) beside its state. **Two
  deployments never share one.**
- **`holon check` says it** (`commands.rs`, `while_it_runs`) for a flow that calls out and both
  reads and writes a collection.

**Durable runs** (`durable.rs`).

- **A resumed run is pinned:** it reads the state as it was when it started, and "its state
  writes apply when it finishes, over whatever is there then" (module docs). `fire_with_state`'s
  `resumed` branch commits without `still_as_read`.
- **The order of a resume** (`resume_parked`): `fire_with_state` (which commits), the log entry,
  the parked record removed, then `persist_deployments`. A host that stops after the commit and
  before the save still has the run parked, and it can be resumed, and commit, again. With whole-
  value writes that is harmless.

**Today's route to a database:** `http.*` with a secret, for databases with an HTTP API
(Supabase's REST, Neon's HTTP queries, Turso, Cloudflare D1). Logged and replayed like any call,
but the manifest names only the host, and there is no transaction across calls.

Two gaps follow:

1. **No store that queries or that deployments share.** A flow that needs "orders over 100 this
   week" lists a collection and filters it, and two flows can't keep one table between them.
2. **A shared folder's commit isn't excluded across deployments.** `still_as_read` and `commit`
   run under the deployment's own state lock. Two deployments that read and write one file of a
   shared folder can both check (each sees the file as it read it) before either writes, then
   both write: the second replaces the first, and neither run knows. The window is the time
   between check and write, which today includes writing the whole file: `land` (`folders.rs`)
   fills the file beside its name **inside** `commit`, then renames it. Slice 1 closes this
   before any database exists.

## 2. A database is a host resource

A host resource (host-resources §3, design) of kind `database`:

```json
{ "resources": {
    "r_51c0aa": { "name": "shop", "kind": "database", "engine": "postgres",
                  "connection": "vault:kv/shop#url", "access": "read-write", "keep": "bytes for 7d" },
    "r_9d2e41": { "name": "ledger", "kind": "database", "engine": "sqlite",
                  "path": "D:\\data\\ledger.db", "access": "read-write", "keep": "hash" } },
  "bindings": { "@server": { "shop": "r_51c0aa" }, "receipt-ledger": { "ledger": "r_9d2e41" } } }
```

- **The flow names it** (`database shop`), as it names a folder; the host binds the name to a
  resource, for the server or one deployment, as `Bound` does for folders. Rebinding is a grant
  change (`grants.rs`), as host-resources §3 has it.
- **`access`** is `FolderAccess` reused: `read` or `read-write`. A deploy that writes a database
  bound `read` is refused before anyone is asked, as `Folders::refuses` refuses a folder write.
- **`keep`** is `folders::Keep` reused: how long a query's rows stay in the log before only their
  hash does (§5).
- **The connection is a secret source,** parsed and fetched as a secret's is
  (`secret_sources.rs`, `Reference::parse`, `Sources::fetch`: `env:HOLON_SECRET_…`,
  `vault:<mount>/<path>#<field>`), into a `Zeroizing<String>` the client holds. It is never in
  the flow, the grant or the log. A SQLite resource has a path instead, checked as a folder's is
  (absolute, links resolved, not inside the server's own data: `Folders::bind`).
- **Limits are not the resource's:** a run's are granted (§5, `DbLimits`).
- **When it's bound, the host reads the schema once** (tables and their primary keys) for the
  manifest's check (§4) and for claims (§7.2); a refresh reads it again after a migration.
- **Schema changes are the admin's.** Flows don't run DDL (§13, fork 3).
- **Addresses** (host-resources §4): `db:<id>/<table>`, and `db:<id>/<table>/<key>` for a row
  named by its primary key. A MongoDB collection is a table here.

## 3. In the language

```holon
database shop

rows = shop.query("select id, total from orders where customer = $1 order by id", req.params.id)
one  = shop.one("select total from orders where id = $1", id)       # a record, or nothing
shop.exec("insert into refunds (order_id, amount) values ($1, $2)", id, amount)
shop.exec("update stock set n = n - $1 where sku = $2 and n >= $1", qty, sku, expect: 1)
```

- **Three leaves,** lowered like a collection's (`collection_get`, `collection_put`): `db_query`,
  `db_one` (reads) and `db_exec` (a write), each with the frozen parameters `database` and `sql`.
  Their arms in `capabilities_of` parse `sql` (§4).
- **`query`** answers a list of records, one per row, keyed by column name; **`one`** answers the
  first row's record, or nothing when there is none.
- **`exec`** is an effect that lands when the run finishes (§6), as a collection put does. It
  answers nothing. `expect: n` (a frozen parameter) is how many rows it must change; anything
  else rolls the run's whole commit back (§6).
- **The SQL must be written as text in the flow,** for the reason an `http.*` host must
  (`docs/manual/language/reference.md`, Rules): it is the frozen parameter the capability is derived from. The values
  are **inputs**, bound as `$1`, `$2`, … and never put into the text, so a value can't change what
  a statement does.
- **One statement per call.** `query` and `one` take a `select` (with `with`); `exec` takes an
  `insert`, `update` or `delete`, without `returning` (an `exec` answers nothing). Refused at
  compile, with the reason: DDL, `begin`/`commit`, locking clauses (`for update`, `lock table`),
  several statements, and calls to functions the engine doesn't mark as having no side effects.
- **Values.** Text, numbers, true/false and none map as they read. Exact decimals (`numeric`,
  `decimal`) come out as text, so a total is never rounded; timestamps as RFC 3339 text; JSON as
  a value; bytes as hex. A parameter takes a Holon value the same way back.
- **MongoDB** has its own leaves (`db_find`, `db_find_one`, `db_insert`, `db_update`,
  `db_delete`), the collection a frozen parameter, the filter or update a JSON literal with `$1`,
  `$2` placeholders (slice 7).

## 4. What the manifest says

**Two capabilities,** one name each like the rest of `Capability`: `DbRead("shop/orders")` and
`DbWrite("shop/orders")`, the database's name for the flow, then the table (a schema-qualified
table keeps its dot: `shop/public.orders`). They work with what exists: `Permit::allows` and
`Permit::named("DbRead")`, the policy's `auto` (`DbRead(*)`), and two new lines in `describe.rs`
("read table "orders" of database "shop"", "write table …").

**Derived from the SQL** by a parser in core (a SQL parser crate, so the wasm module has it and
the editor shows the same manifest):

| Statement | Capabilities |
|---|---|
| `select … from t join u …` | `DbRead` of `t` and `u` |
| `insert into t …` | `DbWrite` of `t` |
| `update t … where …` | `DbWrite` and `DbRead` of `t` (it reads its `where`) |
| `delete from t …` | `DbWrite` and `DbRead` of `t` |
| a subquery, a CTE, a view | `DbRead` of each table or view it names |

- **`derives_state` adds `DbRead` and `DbWrite`** (`deploy.rs`): a database's reads are checked,
  and its writes committed, as a folder's are, so such a deployment gets a state cell.
- **Enforced when performed, twice.** The host parses each statement again before sending it and
  refuses one whose tables the permit doesn't allow, as every effect is refused outside the
  permit (`permissions.rs`, module docs). And the setup guide advises connecting as a database
  role with only the access the resource has, so the database refuses too.
- **Tables, not columns,** at first (§13, fork 2).

## 5. Reads are the run's input

- **Answered through `RunWaits`:** a new `databases` reader beside `collections` and `files`,
  made per attempt in `run_attempt` (`firing.rs`) for the tables the permit lets it read. Each
  query goes in `reads` with its request (`{database, sql, params}`) and answer, and, like a
  file's `Seen`, its rows' hash and count and where their text is kept.
- **Logged and kept as a folder's reads are:** `LoggedRead` with the rows inline while small,
  else in the blob store (`blobs.rs`), for as long as the resource's `keep` says, then only the
  hash. Replay answers from the log, so a run replays, and `holon verify` checks it, with no
  database at hand.
- **Limits:** a `DbLimits` beside `FileLimits` (`permissions.rs`): rows and bytes a run reads,
  each statement's time, statements per run, with defaults and ceilings and named in
  `LIMIT_NAMES` so `[limits]` and grants set them as they set the others. A read past one **fails
  the run** through `waits.failed`, as a file too big does (`READ_FAILED`); it is never cut short,
  since a flow that sums a truncated list is wrong without knowing.
- **A run doesn't see its own writes:** they land when it finishes, as state writes do.
- **Reads during a run are separate statements** at the engine's default isolation. What makes
  the run's view consistent is the check at commit (§6).

## 6. Writes land when the run finishes, in one transaction

**Where:** in `fire_with_state`, after `still_as_read` passes and before `commit`, under the
state lock that check holds. So a database commit happens only once the run's other reads are
known good, and a failed database check sends the run round the optimistic loop with nothing
applied anywhere. Once the database has committed, nothing after it can send the run round
again: the state lock is held and `commit` doesn't check.

**The transaction:**

1. **Begin** (§8 per engine).
2. **Check:** run each of the run's database reads again and compare its hash with the logged
   one (on SQL engines with `for share`, so a row checked stays as checked until the commit). One
   differs: **roll back** and run again, as when `still_as_read` fails.
3. **Apply** each `exec` in the order the run made them. One whose `expect` isn't met: **roll
   back**, and report the writes as not landed with the statement and the count found, as
   `not_all_written` reports a file that couldn't be written. It is not run again: an unmet
   `expect` is the data saying no (no stock), not a race.
4. **Mark:** insert the run's id into the resource's `holon_commits` table, in the same
   transaction.
5. **Commit,** then `commit` applies the run's state, collection and file writes as today.

**Why the mark.** Holon's own writes are whole values, so a run applied twice changes nothing;
an `exec` like `n = n - $1` changes again. The one path that applies a run twice is a resume
after a stop (§1: `resume_parked` commits before it saves that the run is no longer parked).
With the mark, the second commit finds the run's id and applies nothing. A **client** that
retries sends a new run, with a new id: the mark can't see that, and an idempotency key can (the
`idempotent` sample's pattern).

**More than one database:** each commits as above, in resource-id order, before `commit`. There
is no two-phase commit (§13, fork 8): if the host stops between two, the run's next try (a
resume) finds the committed ones marked and applies the rest.

The check (step 2) also catches writers outside Holon: an app that changed a row the run read
sends the run round again, whoever it was.

## 7. How runs share what they write

Three scopes, by how long something is held:

| Scope | Held | Default | For |
|---|---|---|---|
| **Commit lock** on a shared resource | the check and the writes: milliseconds | **on**, every shared resource | two runs never both pass a check and both write |
| **Claim** on what a run read in what it may write | from its first call out to its commit | **on** (built for collections) | one side effect per row, item or file |
| **Run lock** on a resource | the whole run | **off**; an admin turns it on per resource | reads that can't be checked again, writes that can't be made atomic, heavy contention |

Readers never wait for readers, and nothing is held while a run waits on an outside API (but a
claim on exactly the keys it read) or on a person.

### 7.1 Commit locks

Today the commit's lock is the deployment's (`cell.state.lock()`), enough for what only one
deployment touches. A shared resource needs its own, held from the check to the last write, by
every deployment that writes it or checked a read of it:

- **Per kind:**
  - a database: **its own transaction** is the lock (`begin immediate` on SQLite; `for share`
    re-reads and the writes' row locks on Postgres and MySQL);
  - a folder, one server: a lock per **file** in the server (a table beside `Folders`, by the
    file's real path), shared for a file the run read and exclusive for one it writes, with the
    folder held by intent (hierarchical locking: `locks.rs`). A run that writes `a.txt` never
    waits for one that read `b.txt`. A listing holds the folder itself, shared, since a file
    created under its prefix would change it; so does the last try (it can't know before it runs
    which files it touches), for its whole run;
  - a folder several servers bind (a share): also an OS lock on a lock file in it, whole-folder
    (an OS lock is on a whole file: a reader of any file shares it, a writer of any holds it);
  - a deployment's own state and collections: the state lock, as today.
- **Several resources:** taken in resource-id order, after the state lock, so two commits can't
  deadlock.
- **The slow part before the lock.** `land` (`folders.rs`) is split: the fill (writing the file
  beside its name, synced) happens before the lock; under it, only the check and the rename (or,
  create-only, the link). A database's writes are its statements, sent under it.
- **Bounded:** a commit that can't take its locks within `commit_wait` (a new limit in `Limits`,
  default 2 s) is treated as a failed check and runs again. The attempt that runs holding the
  state lock waits for them too, with the same bound; past it, it fails naming the resource.

**This is slice 1, for folders alone.** It closes §1's second gap and needs no database.

### 7.2 Claims

The same rule as `Claiming::keys_of` (what a run read in what it may write), on more kinds of key:

- **`ClaimKey::Row { resource, table, key }`:** a read whose `where` pins the table's primary key
  by equality to parameters (from the schema read at bind, §2), in a table the permit lets it
  write (`DbWrite`).
- **`ClaimKey::Table { resource, table }`:** any other such read (a range, a join, an
  aggregate). It conflicts with every row under it, as `ClaimKey::Collection` does with its items
  (`conflicts`).
- **`ClaimKey::File { resource, name }`:** a file read in a folder the permit lets it write
  (`FileWrite`); a listing claims the folder. Files aren't claimed today.
- **Where they live.** Today's table is in each deployment's `StateCell`, and a claim on a shared
  resource must be seen by every deployment bound to it. So claims on shared resources go in a
  table per resource in the server (beside the commit locks), and for a database several servers
  use, in the database: an advisory lock (Postgres `pg_advisory_lock`, MySQL `GET_LOCK`) on a
  hash of the address, or for SQLite and MongoDB a `holon_claims` table with a lease, so a server
  that stops lets go.
- **`while_it_runs`** (`commands.rs`) says them too: rows for a database, files for a folder.
- **Built (slice 3):** the table is `Folders::claims`, keyed by each resource's real address (a
  database's file, a file's path in lower case) so one database bound under two names is one.
  The primary key is read from the database (`pragma_table_info`) when a run claims; core's
  `sql::pins` finds the columns a `where` sets equal to parameters. A key's values compare as a
  claim normalizes them (`7` and `"7"` are one row). An insert that doesn't give its primary key
  writes `ClaimKey::NewRow`, which waits for a claim on its table but not on its rows. The last
  try holds every table and folder it may touch for its whole run.
- **Built (slice 4):** a claim on a Postgres or MySQL database is taken in the server's table,
  then in the database, on a connection of its own per run (so a server that stops lets go with
  its connection), and released in the same order. Postgres: advisory locks
  (`pg_try_advisory_lock[_shared]`) on the first 8 bytes of `blake3("holon\0" + key)`, a row
  claim holding its table shared and its row exclusive. MySQL: not `GET_LOCK`, which has no
  shared mode, but a row of `holon_claims (k char(32) primary key)` per key, made with
  `insert ignore` and locked `for share nowait` / `for update nowait` in a read-committed
  transaction on the claim's connection. Both are tried every 20 ms until the claim's wait runs
  out. The commit holds the same claims on the shared keys it writes, under the run's id, from
  its check to its last write. Not built: claims across servers on one SQLite file (a
  `holon_claims` table with a lease); SQLite stays one server's.

### 7.3 Run locks (an admin's setting)

`"lock": "run"` on a resource: every run that touches it takes, **before it starts**, a shared
lock if its manifest only reads the resource and an exclusive one if it writes it, derived from
the manifest, in resource-id order, held until it commits, fails or parks.

- **When it's needed:** reads that can't be checked again (a queue, a sequence's next value),
  writes that can't be made atomic with a check (an API with no transactions, a file a legacy
  program expects one writer for), or contention so high that runs going again cost more than
  waiting.
- **The cost, said where it's set:** every run that writes the resource goes one at a time, for
  its whole length, across every deployment bound to it.
- **Bounded as a claim is:** the wait counts against `claim_wait`; past it a request is turned
  away as busy, a 503 naming the resource, as `busy_after_claim_wait` does for a key, and a clock
  tick is skipped and logged.

### 7.4 Durable runs

A run that parks holds no commit lock, claim or run lock while it waits (claims already let go
when it parks). The resumed branch of `fire_with_state` commits without checking, which is the
pinned run's rule for its own state ("over whatever is there then"), and is **not** safe for a
shared resource another deployment may have changed while it waited. For shared resources, a
resumed run's commit checks its reads like any other (§6, step 2). When they changed, it runs
again **from where it last waited**, with fresh reads: the events delivered and the requests it
made are answered from its record (the parked record's `events` and `rounds`), as an optimistic
run's `prior` is (§13, fork 10).

## 8. Engines

| Engine | Client | Commit | Claims across servers |
|---|---|---|---|
| SQLite | embedded (bundled) | `begin immediate`; the file's own locking across processes | `holon_claims` table, leased |
| Postgres | a pooled client per resource | `read committed` + `for share` re-reads (`serializable` not needed); built: `serializable` | `pg_advisory_lock` |
| MySQL | a pooled client per resource | `repeatable read` + `for share` re-reads; built: `serializable` | `GET_LOCK`; built: locks on `holon_claims` rows |
| MongoDB (Atlas too) | the driver, `mongodb+srv://` | a transaction (needs a replica set, which Atlas is) | `holon_claims` collection, TTL |

- **SQLite first:** no network, no service to run in CI, one file the host already knows how to
  confine. It gives queries, indexes and a store several deployments share on day one.
- **MongoDB through the driver,** not HTTP: MongoDB retired Atlas's HTTP Data API.
- **Databases with an HTTP API** stay reachable through `http.*`, unchanged.
- **Built (slice 4):** `postgres` (sync, TLS by rustls with the public CAs) and `mysql` (pure
  Rust, rustls). A binding's connection is a secret source (`env:HOLON_SECRET_…`, `vault:…`),
  fetched as secrets are; the binding keeps the source and the address without credentials,
  which keys its claims and commit lock. Parameters cross as text on Postgres (booleans and
  numbers adjusted to the type Postgres infers), and `$n` becomes `?` for MySQL
  (`sql::positional` in core). Each read runs in a read-only transaction with the statement
  limit. Values back: `numeric`/`decimal` as text, times as RFC 3339 in UTC, `json` as a value,
  bytes as hex; any other type fails, saying to cast it. Sessions are set the same way on both:
  standard strings, UTC, and on MySQL `ANSI_QUOTES`, `PIPES_AS_CONCAT`, `NO_BACKSLASH_ESCAPES`,
  so a flow's SQL means one thing everywhere.
- **Deviation (slice 4): the commit is `serializable`,** not `read committed` with `for share`
  re-reads: on Postgres `for share` needs `update` on the table, which a role that only reads it
  shouldn't have. The check still re-reads; serializable is what makes it hold. Serialization
  failures, deadlocks and a lock not had within `commit_wait_ms` (Postgres 40001, 40P01, 55P03;
  MySQL 1205, 1213, 3572) run the run again. What it costs: on Postgres, a program outside Holon
  that writes a checked row in the milliseconds between the check and the commit isn't seen
  (Holon's own runs are). A database's transaction commits before the run's state and files
  land; once one has committed, a later one that fails is reported as not landed.

## 9. Seeing it

- **The run's log** (`LogEntry`): each query in `reads` (statement, parameters, row count, hash,
  rows while kept), each `exec` in `performed` with the rows it changed, `retries` and why ("row
  1042 of `shop/orders` changed before it could commit"), `claims` with the new keys, and the
  commit's wait for its locks.
- **The timeline** (host-resources §5, design): `read`, `write` and `claim` rows under
  `db:<id>/<table>`, each pointing to its run.
- **The editor:** the deploy dialog lists tables (through `describe.rs`); the inspector shows a
  query node's rows for the selected run; the admin page defines database resources, the
  connection by secret source only.

## 10. Samples

Each slice ships with its sample (CLAUDE.md), run live before its commit and deployed by the
end-to-end tests:

- **slice 1, `shared-ledger`:** two deployments add lines to one file of a shared folder, called
  at once, many times; every line lands. Without the slice, some are lost.
- **slice 2, `orders-api`:** a small REST API over a SQLite table: list with a filter, get one,
  create, its manifest naming the table.
- **slice 3, `refund-once`:** two refund requests for one order at the same moment call the
  payment API (an httpbin stand-in) once; the second reads the order as refunded. The database
  form of `idempotent` and `cache-aside`.
- **slice 4:** `orders-api` on Postgres, the same text, the resource's engine changed.
- **slice 7, `atlas-notes`:** notes kept in a MongoDB collection.

## 11. Tests

- **Core** (`kind.rs`): statements to capabilities (joins, subqueries, CTEs, views, an
  `update`'s read); each refused form refused with its reason; SQL from a value refused at
  compile; the wasm parity test (`holon-wasm`) sees the new leaves.
- **Commit locks** (`tests.rs`): two deployments writing one shared file at once, many times: no
  write lost, each run committed or ran again. Locks across resources taken in id order (a
  property test over random resource sets: no deadlock).
- **Databases:** a read changed between the run and its commit sends it round again with its
  requests answered from its record; an unmet `expect` rolls back everything the run wrote there;
  a resume after a stop between commit and save applies nothing twice (`holon_commits`).
- **Claims** (`claims.rs`): a primary-key read claims the row, a range read the table; a table
  claim waits for row claims under it; claims of two deployments on one resource exclude each
  other.
- **Run locks:** a resource set to `run` serializes writers and lets readers through together; a
  request past `claim_wait` answers 503 naming it.
- **Durable:** a resumed run whose shared read changed runs again from its last wait, its events
  kept.
- **Replay and verify** of a run that queried, with the database gone.
- **Engines:** SQLite always; Postgres and MySQL in CI as services; MongoDB against a local
  replica set.

## 12. Slices

1. **Commit locks on shared folders** (§7.1): `land` split, a lock per folder, taken in id order.
   Closes today's gap; no new language. Sample `shared-ledger`.
2. **Database resources, SQLite:** the `database` header, `db_query`/`db_one`/`db_exec`,
   `DbRead`/`DbWrite` from the SQL, `derives_state`, reads through `RunWaits` and kept,
   `DbLimits`, the commit transaction with its check and `holon_commits`. Needs a resource to
   bind (host-resources' registry, its first slice, or a `--databases` file shaped like
   `--folders` until then). Sample `orders-api`.
3. **Claims on rows, tables and files,** in a table per shared resource (§7.2). Sample
   `refund-once`.
4. **Postgres and MySQL,** with claims in the database for several servers.
5. **Run locks** (§7.3).
6. **Resumed runs check their shared reads** (§7.4).
7. **MongoDB,** Atlas included.

## 13. Forks, surfaced

1. **SQL as text, or queries built in the language?** Text. People and agents already write SQL,
   it parses into exactly what the manifest needs, and a builder would be a second query
   language to learn and keep.
2. **Permissions by database, table or column?** Table. A database is too coarse to approve ("may
   write anything in `shop`"); columns can follow when someone needs "may read `customers` but not
   `email`".
3. **DDL in flows?** No. A flow that changes a schema other flows depend on is a migration, and
   migrations are the admin's, run with their own tools; the host reads the schema again after.
4. **Writes as they run, or when the run finishes?** When it finishes, in one transaction. It is
   what state does, a failed run then writes nothing, and the check at commit is what makes
   optimistic runs correct.
5. **A run reads its own writes?** No: they haven't landed. Same as state.
6. **`serializable` commits, or checking reads again?** Checking reads again under `for share`.
   It works the same on every engine, catches writers outside Holon, and costs one re-read per
   read, only at commit. Built: both, the re-reads in a `serializable` transaction (§8), since
   `for share` needs write permission on Postgres.
7. **Exact decimals as numbers or text?** Text. A total that comes back rounded is a bug nobody
   sees.
8. **Atomic across resources (two-phase commit)?** No. Ordered commits, each marked with the run's
   id, give "every resource gets the run's writes once"; two-phase commit would need every engine
   and every folder to take part, and a host that stops mid-way would hold others' locks.
9. **Commit locks on by default; run locks off.** Commit locks cost milliseconds and fix lost
   writes; run locks serialize whole runs and are for the resources that need them.
10. **A resumed run whose shared read changed: fail, or run again?** Run again from its last wait,
    with fresh reads. Failing would throw away a person's answer over a race they had no part in.
11. **Claims for several servers: in the database, or a lock service?** In the database: it is
    the one thing all of them already reach, and it lets go when a connection drops. Built for
    Postgres and MySQL (§7.2).
12. **A primary key: from the schema, or declared in the flow?** From the schema, read at bind
    and on refresh. A flow that states its table's key could state it wrong.
13. **A read past a limit: cut short, or fail?** Fail, naming the limit, as a file too big does.
14. **Slice 2 before host resources?** If host-resources' registry isn't built by then, a
    `--databases` file shaped like `--folders` (a name, an engine, a connection source, access,
    keep), migrated into resources when they land, as folder bindings are (host-resources §3).

## 14. What stays true

- **Every run ends:** every wait (commit locks, claims, run locks, statements) has a bound, and
  past it the run goes again or fails, saying why.
- **What a flow may do is derived from it, never declared:** tables from its SQL, claims from its
  reads, run locks from its manifest.
- **A run replays without the outside world:** its rows are in its log.
- **Secrets stay out of runs:** a connection string is fetched by the host and never logged.
- **No deadlock:** the state lock, then resource locks in id order; and a run never waits for a
  claim while holding one.
