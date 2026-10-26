# Databases on the host

A flow can read and write a SQL database on the server's machine, but only one an admin has
bound to a name, and only by that name. The flow says `database shop`; the admin decides that
`shop` is `/srv/shop.db`, and what its tables are. The flow never sees a path, runs no DDL, and is
granted each table its SQL touches, no more.

Today a database is a SQLite file. Postgres, MySQL and MongoDB are designed
(`docs/L4s/databases-L4.md`) and not here yet.

## Binding a database

An admin binds databases from the editor's **admin** page (**databases**), or on the server's
machine:

```sh
holon database add shop ~/shop.db --schema "create table orders (id integer primary key, customer text not null, total numeric not null, refunded integer not null default 0)"
holon database add reports /srv/reports.db --read-only
holon database list
holon database remove shop
```

| Option | |
|---|---|
| `--schema SQL`, `--schema-file FILE` | run this first (`create table …`), creating the file if there is none |
| `--read-only` | flows may read it, not write it |
| `--for SLUG` | for that deployment only: it sees this database as the name, and no other deployment sees it |

- A name is letters, digits, `-` and `_`, up to 64 characters.
- Without a schema, the file must already be a SQLite database.
- The server's own data folder can't hold one.
- A writable database gets a table `holon_commits`, where each run that writes marks itself done
  (see below). Flows can't read or write it.
- Removing a binding leaves the file where it is; deployments that use it stop until it is bound
  again.

A deploy is refused until each database the flow names is bound, and bound read-write where the
flow writes.

## Reading and writing

```holon
flow orders-api
port 8150
database shop

route POST /orders (req) {
  shop.exec("insert into orders (customer, total) values ($1, $2)", req.json.customer, req.json.total)
  respond { placed: req.json.customer }, status: 201
}

route GET /orders/:customer (req) { respond shop.query("select id, total, refunded from orders where customer = $1 order by id", req.params.customer) }
```

| Written | Is |
|---|---|
| `db.query(sql, v1, v2, …)` | the rows, as a list of records, as the run began |
| `db.one(sql, v1, …)` | the first row, or no value when there is none (a route answers 404) |
| `db.exec(sql, v1, …)` | run the statement (`insert`, `update`, `delete`) when the run finishes |
| `db.exec(sql, v1, …, expect: n)` | the same, only if it changes exactly `n` rows: else nothing the run wrote lands, and a route answers 409 |

- The SQL is written as text in the flow, because its tables are what the deployment is granted:
  `select … from orders` asks `DbRead("shop/orders")`, `update orders …` asks
  `DbWrite("shop/orders")`. SQL from a value is an error that says so, and so is a statement that
  names no table.
- Values go in after the SQL, as `$1`, `$2`, …; never into its text. The count must match.
- A number crosses as a number, `true`/`false` as 1/0, no value as `null`, a record or list as its
  JSON text. A column that is `null` is a missing field; an integer past 2^53 comes back as text,
  a blob as hex.
- Reads are of the database as the run began; writes land when it finishes, all of them or none.

## When runs touch the same rows

A run's reads and its writes are one step. When the run finishes, the server opens a transaction
on each database it wrote, reads again what the run read, and lands the writes only if it is all
as the run found it. If another run changed it in between, the run runs again, on what is there
now (four times, then once more holding the database so that no other run can change it). Two
deployments counting in one row lose no count.

`expect:` is for the change that must happen once: refund an order only if it isn't refunded yet.

```holon
flow refunds
port 8151
database shop

route POST /order/:id/refund (req) {
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", req.params.id, expect: 1)
  respond { refunded: req.params.id }
}
```

The second refund changes no row: the run fails with `changed 0 rows, not 1 row`, writes
nothing (here or anywhere else), and answers 409.

The same transaction inserts the run's id into `holon_commits`, so a database says itself which
runs landed in it.

## Calling out between a read and a write

Running again is safe for what a run writes, not for what it sends: a run that read an order,
called a payment API and then found the order changed would have paid twice by the time it ran
again. So a run that reads a row it may write, and then calls out, **holds the row** from its
first outbound call until it commits. Another run that read the same row waits at its own first
call; when the row is let go, it reads it again, and runs again on what it finds, having sent
nothing.

```holon
flow refund-once
port 8151
database shop

route POST /refund/:id (req) {
  order = shop.one("select id, total from orders where id = $1 and refunded = 0", req.params.id)
  paid = http.post("httpbin.org", "/anything/refunds", headers: { "content-type": "application/json" }, body: json({ order: order.id, amount: order.total }))
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", order.id, expect: 1)
  shop.exec("insert into refunds (order_id, amount, status) values ($1, $2, $3)", order.id, order.total, paid.status)
  respond { refunded: order.total, payment: paid.status }, missing: 409
}

route GET /refunds/:id (req) {
  respond shop.query("select amount, status from refunds where order_id = $1", req.params.id)
}
```

Two refunds of one order at once call the payment API once: the second finds the order refunded,
its query answers no row, nothing after it runs, and it answers 409. The `refund-once` sample
shows it beside `orders-api`, on the same database.

- A read whose `where` sets the table's whole primary key equal to values (`id = $1`, with other
  conditions joined by `and`) holds that row. Any other read of a table it may write (a range, a
  join, a count) holds the whole table, which waits for every row under it.
- An insert waits only for a run holding the whole table; one that gives the primary key waits
  for that row too.
- Holding is across deployments: every deployment bound to the database sees the same rows held,
  whatever name it knows the database by.
- A run that never calls out holds nothing; one that only reads a table holds nothing in it.
- `holon check` says it, under `while it runs:`, and the run log lists each row held, how long it
  waited, and whether it found the row changed. A run waits `claim_wait_ms` at most (10 s).

## Limits

| Name | Is | Default | Ceiling |
|---|---|---|---|
| `max_db_rows` | rows a run reads in all | 10000 | 1000000 |
| `max_db_read_bytes` | bytes a run reads in all | 16777216 (16 MiB) | 268435456 |
| `db_statement_ms` | time one statement may take | 5000 | 60000 |
| `max_db_statements` | statements a run sends, reads and writes | 100 | 10000 |

A read past a limit fails the run, and it writes nothing. A deployment that needs more asks for
it, as with every limit ([Permissions](../reference/permissions.md#limits)).

## What the run log keeps

Each read is in the run's log with its answer, so `holon verify` runs it again offline, with no
database: the same rows give the same answer. A run that failed (an unmet `expect:`) wrote
nothing and is skipped.
