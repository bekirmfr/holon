# Databases

SQL and document databases an admin binds on the host, which a flow reads and writes by a name,
by the tables its SQL names. The design, its slices and what each built differently is the
[databases L4](../../L4s/databases-L4.md) (DB); claims come from the
[key claims L4](../../L4s/key-claims-L4.md); the user's side is the manual's
[databases guide](../../manual/guides/databases.md).

## 1. What it is

```
flow orders-api
database shop

route GET /order/:id (req) {
  respond shop.one("select id, customer, total, refunded from orders where id = $1", req.params.id)
}

route POST /order/:id/refund (req) {
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", req.params.id, expect: 1)
  respond { refunded: req.params.id }
}
```

`database shop` names the database; `shop.query(sql, v…)` is `db_query{database: shop, sql}`
(the rows as the run began), `shop.one` `db_one` (the first row, or no value), `shop.exec`
`db_exec` (run when the run finishes, and with `expect: n` only if it changes `n` rows). The SQL
is a text literal: core parses it (`holon-core/src/sql.rs`) and derives `DbRead("shop/orders")`
and `DbWrite("shop/orders")` from the tables it names; the values are `$1`, `$2`, … and never
enter the text. A MongoDB is read and written with `notes.find(c, { … })`, `find_one`, `insert`,
`update` and `delete`, its records frozen into templates at compile (`holon-core/src/documents.rs`)
([LANGUAGE.md](../../LANGUAGE.md) §5.4).

- `crates/holon/src/databases.rs`: bindings (`Databases`, `Database`), reads (`Readable`,
  `DbLimits`), the commit (`commit`, `Commit`), claims of reads and writes, `holon database`.
- `engines.rs`: Postgres and MySQL; `mongo.rs`: MongoDB; SQLite is `rusqlite`, in databases.rs.
- `claims.rs`, `locks.rs`: claims on rows and tables, commit and run locks, shared with folders.

## 2. How it works

- **Bound by an admin.** `holon database add NAME PATH|SOURCE [--read-only] [--schema SQL |
  --schema-file FILE] [--lock run] [--for SLUG]`, `list`, `remove`, `tables` (or the admin page's
  resources), kept in `databases.json` (`--databases`). A SQLite database is a file on the server's
  machine, checked as a folder's path is (never inside the server's data); a Postgres or MySQL
  database a connection whose URL the host fetches from a secret's source (`env:HOLON_SECRET_…`,
  `vault:…`), never stored, shown or logged; a MongoDB the same, never with a schema. The admin's
  schema runs at bind; a flow runs no DDL. A writable database gets `holon_commits`.
- **Reads** are answered on the spot, from the tables the deployment was granted (the host parses
  the statement again before sending it), within `DbLimits`: 10,000 rows, 16 MiB, 5 s a
  statement, 100 statements a run (ceilings 1,000,000, 256 MiB, 60 s, 10,000). A read past one
  fails the run (`READ_FAILED`), never cut short. Each is kept with the run's reads, rows and all,
  so replay and `holon verify` need no database. On Postgres and MySQL each read is a read-only
  transaction with the statement limit.
- **Values.** Exact decimals come out as text, times as RFC 3339 in UTC, JSON as a value, bytes as
  hex; any other type fails, saying to cast it. Sessions are set alike on every engine so a flow's
  SQL means one thing (`$n` becomes `?` for MySQL, `sql::positional`).
- **The commit.** In `fire_with_state`, after `still_as_read` passes and under the state lock,
  `databases::commit` opens one transaction per database, in the order of their names: runs each
  of the run's reads there again and compares (changed: `Commit::Again`, the run goes round
  again); applies each `exec` in order, checking `expect` (unmet: `Commit::Refused` with `UNMET`,
  nothing written, the route answers 409, not run again); marks the run's id in `holon_commits`
  (a run marked already applies nothing). Postgres and MySQL commit `serializable`, a
  serialization failure, deadlock or lock timeout sending the run round again; MongoDB commits a
  transaction (it must be a replica set). The SQLite transactions stay open while the run's state,
  collections and files land (`then`), then commit.
- **Shared with other runs** ([DB](../../L4s/databases-L4.md) §7): commit locks for the check and
  the writes (the database's own transaction, `begin immediate` on SQLite); claims, from a run's
  first call out to its commit, on the rows it read by primary key in tables it may write
  (`ClaimKey::Row`, the key read from the schema, `sql::pins`) or the whole table for any other
  read (`ClaimKey::Table`), an insert without its key `ClaimKey::NewRow`. They live in the
  server's table (`Folders::claims`, by the database's real address) and, for several servers, in
  the database: Postgres advisory locks, rows of `holon_claims` on MySQL, `holon_claims` documents
  with a 30 s lease on MongoDB, each asked again every `claims::POLL` (20 ms). `--lock run` holds a
  database for whole runs (`RunLocked`), shared for a reader, exclusive for a writer, past
  `claim_wait` a 503.
- **Durable runs** check their shared reads at commit as any run does, and run again from their
  last wait when one changed (DB §7.4).

## 3. Why it is this way

- **SQL as text** (DB §13, fork 1): people and agents write it already, and it parses into what
  the manifest needs. **Tables, not columns** (fork 2): a database is too coarse to approve.
- **Writes when the run finishes, in one transaction** (fork 4): what state does; a failed run
  writes nothing, and the check at commit makes optimistic runs correct.
- **Re-read at commit**, not only isolation (fork 6): it works on every engine and catches writers
  outside Holon. Built as re-reads inside `serializable`, since `for share` needs write permission
  on Postgres (DB §8, deviation).
- **The mark** (`holon_commits`): `n = n - $1` applied twice changes twice; a resume after a stop
  is the one path that commits a run twice (DB §6).
- **An unmet `expect` is the data saying no**, not a race: refused, not run again.
- **Claims in the database for several servers** (fork 11): it is the one thing they all reach,
  and it lets go when a connection drops.
- **Exact decimals as text** (fork 7): a rounded total is a bug nobody sees. Holon's numbers are
  64-bit floats ([host capabilities L4](../../L4s/host-capabilities-L4.md) §1).
- **Run locks off by default** (fork 9): commit locks cost milliseconds; run locks serialize whole
  runs, for a sequence's next value, a queue, or contention so high that running again costs more.

## 4. Reach and limits

- **No two-phase commit** across databases (fork 8): each commits in turn, marked; a host stopped
  between two leaves the next try (a resume) to apply the rest. A Postgres or MySQL database
  commits before the run's state and files land; one that fails after is reported as not landed.
- **A writer outside Holon** that changes a checked row between the check and the commit isn't
  seen on Postgres, nor a document on MongoDB (snapshot isolation); Holon's own runs are held off by
  claims (DB §8).
- **Reads are logged whole**: a binding's `keep` isn't built, so a large result makes a large line.
- **SQLite is one server's**: no claims or run lock across servers on one file.
- **Rebinding isn't a grant change**: unbinding pauses nothing (calls fail, "isn't bound"), and a
  binding changes what the next runs reach without asking (DB §9.1).
- **A secret in a query's parameters** goes to the database the admin bound, with no run-time value
  check as a request has ([secret destinations L4](../../L4s/secret-destinations-L4.md) §8).
- **Nothing is measured**: no number here for a read's, a claim's or a commit's cost.

## 5. What the tests hold

- Core `sql.rs`: `a_select_reads_every_table_it_names`, `a_write_writes_its_table_and_reads_what_it_matches`,
  `what_isnt_a_read_or_a_write_of_rows_is_refused_saying_why`, `comments_and_strings_hide_nothing`,
  `a_where_pins_the_columns_it_sets_equal_to_parameters`, `dollar_parameters_become_question_marks_in_order`;
  `kind.rs`: `database_leaves_name_the_tables_their_statement_touches`.
- databases.rs: `values_cross_as_their_columns_say`,
  `a_database_is_bound_by_an_admin_with_its_schema_and_never_inside_the_server`,
  `a_primary_key_read_claims_its_row_and_any_other_the_table`; engines.rs:
  `exact_decimals_and_times_read_as_text`, `a_connection_names_its_database_without_its_credentials`;
  locks.rs: `locks_taken_in_address_order_never_deadlock`.
- tests.rs: `a_flow_reads_and_writes_a_bound_database` (an unmet `expect` lands nothing and
  answers 409), `two_deployments_counting_in_one_row_lose_no_count`,
  `two_deployments_refunding_one_order_call_the_payment_api_once`,
  `a_row_another_program_changed_sends_the_run_round_again`,
  `a_database_held_for_whole_runs_runs_its_writers_one_at_a_time`,
  `a_resumed_run_resumed_again_after_its_commit_applies_once`,
  `approved_payout_keeps_the_deposits_made_while_it_waited`; with `HOLON_TEST_POSTGRES` and
  `HOLON_TEST_MYSQL` (CI's services): `orders_api_runs_on_postgres_and_mysql_as_on_sqlite`,
  `two_servers_refunding_one_order_call_the_payment_api_once`,
  `a_database_held_for_whole_runs_holds_across_servers`; with `HOLON_TEST_MONGODB`:
  `atlas_notes_keeps_notes_as_documents_on_mongodb`, `a_document_that_changed_before_its_commit_runs_again`,
  `mongodb_holds_run_locks_shared_and_exclusive_across_servers`.
- **Samples**: `orders-api` (SQL, `expect`), `refund-once` (claims), `ticket-numbers` (run locks),
  `approved-payout` (a waiting run checks at commit), `atlas-notes` (MongoDB), `license-check` (a
  key in a query). All deploy in `samples.e2e.ts` (`atlas-notes` only where `HOLON_TEST_MONGODB`
  is set, as in CI); `orders-api`, `ticket-numbers`, `atlas-notes` and `license-check` have tests
  there, `refund-once` and `approved-payout` in `system.e2e.ts`.
- **Gaps**: the end-to-end suite runs the SQL samples on SQLite only; a test without its
  environment variable runs on no remote engine and passes, saying so.

## 6. History

Databases arrived in rc.27 (slices 1–7: commit locks, SQLite, claims, Postgres and MySQL, run
locks, resumed runs, MongoDB; CHANGELOG), and SQL brackets read their tables in rc.28. This
repository's history starts after that, at two root commits holding the whole tree (`6bc5c68`,
the rc.29 release, and `e49e371`). Since:

| Commit | Change |
|---|---|
| `e49e371`, `e5dd456` | CI's MongoDB pinned by digest, then 7.0 |
| `7d3cab7` | Binding or unbinding a deployment's own database is an event on its timeline |
| `8f93149` | `holon database tables` |
| `ded3615` | A key in a database read's values is said to go to the database |
| `71b895c` | The server's own secrets, capped to hosts and `db:` databases |
