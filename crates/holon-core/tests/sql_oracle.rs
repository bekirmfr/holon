// What a SQL statement touches, as `holon_core::sql::tables` reads it, against an engine's own
// account of the same statement. What each test guards, why, and what its failing means is on the
// test; this is how they work.
//
// SQLite is asked. As it prepares a statement it tells an authorizer every table it will read or
// write and every function it will call (`sqlite3_set_authorizer`). The statements are generated:
// each place a table can be named (`READS`, `WRITES`), crossed with each way of spelling one
// (`SPELLINGS`, `TARGETS`) and each way of dressing a statement up (`dressed`), plus the tricks
// that make two readers disagree (`TRICKS`), some 17,000 in all. A statement the scan takes must
// be within what SQLite says, and a plain one (`PLAIN`) exactly it; one the scan refuses is safe by
// refusing. SQLite speaks for the SQL it shares with the others. What only Postgres or MySQL reads
// has no engine here, so it is a table of statements with what they touch written beside them
// (`ELSEWHERE`), held to the same rule.
//
// To add a case: a new place a table can hide is a line of `READS` or `WRITES`, a new spelling a
// line of `SPELLINGS`, a new trick a line of `TRICKS`; none needs its answer written, SQLite gives
// it. Only `ELSEWHERE` carries answers.

use holon_core::sql::{tables, FUNCTIONS};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// What an engine says a statement does.
#[derive(Clone, Debug, Default, PartialEq)]
struct Says {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    calls: BTreeSet<String>,
    /// Anything that isn't a read, a write or a call: a pragma, an attach, a change to the schema.
    other: BTreeSet<String>,
}

fn engine() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "create table orders (id integer primary key, customer, total, status, note);
         create table payments (id integer primary key, order_id, amount);
         create table customers (id integer primary key, name, email);
         create table cards (id integer primary key, pan, token);
         create table audit (id integer primary key, what);
         create index orders_customer on orders (customer);",
    )
    .unwrap();
    db
}

/// What SQLite says `sql` does, or nothing when it doesn't take it (then nothing runs).
fn sqlite_says(db: &Connection, sql: &str) -> Option<Says> {
    let said = Arc::new(Mutex::new(Says::default()));
    let heard = Arc::clone(&said);
    db.authorizer(Some(move |ctx: AuthContext<'_>| {
        let mut says = heard.lock().unwrap();
        match ctx.action {
            AuthAction::Read { table_name, .. } => says.reads.insert(table_name.to_lowercase()),
            AuthAction::Insert { table_name } | AuthAction::Delete { table_name } | AuthAction::Update { table_name, .. } => says.writes.insert(table_name.to_lowercase()),
            AuthAction::Function { function_name } => says.calls.insert(function_name.to_lowercase()),
            AuthAction::Select | AuthAction::Recursive => false,
            other => says.other.insert(format!("{other:?}")),
        };
        Authorization::Allow
    }));
    let prepared = db.prepare(sql).is_ok();
    db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let says = said.lock().unwrap().clone();
    prepared.then_some(says)
}

/// A table as the scan names it, as SQLite does: lower case, without the database it is in, and
/// its schema's table by the name SQLite reports it under.
fn plain(name: &str) -> String {
    let name = name.to_lowercase();
    let name = name.strip_prefix("main.").map(String::from).unwrap_or(name);
    if name == "sqlite_schema" { "sqlite_master".into() } else { name }
}

/// What SQLite calls for an operator, not a function a statement names: `like`, `glob`, `->`.
const OPERATORS: &[&str] = &["like", "glob", "->", "->>"];

/// Whether the scan's reading of `sql` holds against `says`: `Ok(taken)`, or what it missed.
fn holds(sql: &str, write: bool, says: &Says) -> Result<bool, String> {
    let Ok(scanned) = tables(sql, write) else { return Ok(false) };
    let (reads, writes): (BTreeSet<String>, BTreeSet<String>) = (scanned.reads.iter().map(|t| plain(t)).collect(), scanned.writes.iter().map(|t| plain(t)).collect());
    let touched: BTreeSet<&String> = reads.union(&writes).collect();
    let mut missed = Vec::new();
    for t in says.reads.iter().filter(|t| !touched.contains(t)) {
        missed.push(format!("reads {t}"));
    }
    for t in says.writes.iter().filter(|t| !writes.contains(*t)) {
        missed.push(format!("writes {t}"));
    }
    for f in says.calls.iter().filter(|f| !FUNCTIONS.contains(&f.as_str()) && !OPERATORS.contains(&f.as_str())) {
        missed.push(format!("calls {f}()"));
    }
    for o in &says.other {
        missed.push(format!("does {o}"));
    }
    if !write && !says.writes.is_empty() {
        missed.push("writes, sent as a read".into());
    }
    match missed.is_empty() {
        true => Ok(true),
        false => Err(format!("{sql:?}\n    the scan: reads {reads:?} writes {writes:?}\n    missed: {}", missed.join(", "))),
    }
}

/// Where a statement can name a table to read: `{T}` is the table, as a from's item.
const READS: &[&str] = &[
    "select * from {T}",
    "select * from orders, {T}",
    "select * from {T}, orders",
    "select * from orders join {T} on 1 = 1",
    "select * from orders inner join {T} on 1 = 1",
    "select * from orders left outer join {T} on 1 = 1",
    "select * from orders cross join {T}",
    "select * from orders natural join {T}",
    "select * from (orders join {T} on 1 = 1)",
    "select * from orders o join payments p on p.order_id = o.id join {T} on 1 = 1",
    "select id from orders union select id from {T}",
    "select id from orders union all select id from {T}",
    "select id from orders except select id from {T}",
    "select id from orders intersect select id from {T}",
    "select * from orders where id in (select id from {T})",
    "select * from orders where id not in (select id from {T})",
    "select * from orders where exists (select 1 from {T})",
    "select * from orders where id = (select max(id) from {T})",
    "select (select count(*) from {T}) from orders",
    "select (select count(*) from {T})",
    "select case when exists (select 1 from {T}) then 1 else 0 end from orders",
    "select coalesce((select max(id) from {T}), 0) from orders",
    "select * from orders order by (select count(*) from {T})",
    "select * from orders limit (select count(*) from {T})",
    "select * from orders limit 1 offset (select count(*) from {T})",
    "select customer from orders group by customer having count(*) > (select count(*) from {T})",
    "select sum(total) over (order by (select count(*) from {T})) from orders",
    "select * from orders where total between 1 and (select count(*) from {T})",
    "select * from orders where id in (values ((select max(id) from {T})))",
    "select * from orders o join payments p on p.order_id = o.id and exists (select 1 from {T})",
    "select * from orders where customer = $1 and id in (select id from {T} where id > $2)",
    "with r as (select id from {T}) select * from r",
    "with r as (select id from orders), s as (select id from {T}) select * from r, s",
    "with r (n) as (select id from {T}) select n from r",
    "with recursive r (n) as (select 1 union all select n + 1 from r, {T} where n < 3) select n from r",
    "select * from (select * from (select * from {T}))",
    "select * from orders where id in (select id from payments where id in (select id from {T}))",
];

/// Where a write can name a table: `{T}` one it reads, `{W}` the one it writes.
const WRITES: &[&str] = &[
    "insert into audit (what) select id from {T}",
    "insert into audit (what) values ((select max(id) from {T}))",
    "insert or replace into audit (id, what) select id, id from {T}",
    "replace into audit (id, what) select id, id from {T}",
    "insert into audit (id, what) values ($1, $2) on conflict (id) do update set what = (select max(id) from {T})",
    "insert into audit (id, what) values ($1, $2) on conflict (id) do update set what = $2 where exists (select 1 from {T})",
    "update orders set note = (select max(id) from {T}) where id = $1",
    "update orders set note = $1 where id in (select id from {T})",
    "update orders set note = $1 from {T} where orders.id = $2",
    "update or replace orders set note = $1 where exists (select 1 from {T})",
    "delete from orders where id in (select id from {T})",
    "delete from orders where exists (select 1 from {T})",
    "insert into {W} (id) values ($1)",
    "insert or ignore into {W} (id) values ($1)",
    "replace into {W} (id) values ($1)",
    "insert into {W} select * from orders",
    "update {W} set id = $1",
    "update {W} set id = $1 where id = $2",
    "update or ignore {W} set id = $1",
    "delete from {W}",
    "delete from {W} where id = $1",
];

/// Statements of the ordinary kind, each taken and named exactly as SQLite says: joins and lists,
/// subqueries, a with's names, words that look like a from (`extract`'s is Postgres's, below), and
/// comments and strings that name a table without reading it.
const PLAIN: &[(&str, bool)] = &[
    ("select id, total from orders where id = $1", false),
    ("SELECT o.id FROM Orders o JOIN customers AS c ON c.id = o.customer WHERE c.name = $2 AND o.total > $1", false),
    ("select * from orders, payments as p, main.customers where orders.id = p.order_id", false),
    ("select * from orders left outer join payments using (id) natural join customers", false),
    ("select (select max(amount) from payments where order_id = o.id) from orders o", false),
    ("select * from orders where customer in (select id from customers where name)", false),
    ("select * from (select id from orders) sub", false),
    ("select id from orders union select id from payments", false),
    ("select * from (select id from orders) a, customers", false),
    ("select * from orders join payments on orders.id = payments.order_id, customers where customers.id = $1", false),
    ("select * from (orders join payments on orders.id = payments.order_id) left join customers on true", false),
    ("select * from (values (1), (2)) as v", false),
    // A comma past a where, a group or an order starts no table.
    ("select * from orders where total > 1 order by id, customer", false),
    ("select id, customer from orders group by id, customer", false),
    ("select * from orders where id in (1, 2) order by customer, id", false),
    // A with's names aren't tables where they are seen.
    ("with big as (select * from orders where total > $1) select * from big join customers c on c.id = big.customer", false),
    ("with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select n from t", false),
    // A from that names no table.
    ("select cast(total as decimal(10, 2)) from orders", false),
    ("select * from orders where note is distinct from status", false),
    ("select * from orders where note is not distinct from status", false),
    ("select count(*) over (partition by customer order by total) from orders", false),
    // Comments and strings hide nothing, and name nothing.
    ("select 'from cards' as s from orders -- from payments\n", false),
    ("select /* join cards */ note from orders;", false),
    ("select 'it''s' from orders", false),
    // A write writes its table and reads what it matches.
    ("insert into payments (order_id, amount) values ($1, $2)", true),
    ("insert into audit select id, note from orders where id = $1", true),
    ("update payments set amount = amount - $1 where id = $2 and amount >= $1", true),
    ("update orders as o set status = true from payments p where p.order_id = o.id", true),
    ("update orders indexed by orders_customer set note = $1 where customer = $2", true),
    ("delete from payments where amount < $1", true),
    ("insert into audit (id, what) values ($1, 1) on conflict (id) do update set what = audit.what + 1", true),
    ("INSERT OR REPLACE INTO audit (id, what) VALUES ($1, $2)", true),
    ("replace into audit (id, what) values ($1, $2)", true),
];

/// Ways of spelling the table `cards` where a from's item goes. The first three are the plain ones.
const SPELLINGS: &[&str] = &[
    "cards",
    "cards c",
    "cards as c",
    "main.cards",
    "CARDS",
    "Cards AS C",
    "\"cards\"",
    "\"CARDS\"",
    "`cards`",
    "[cards]",
    "main.\"cards\"",
    "\"main\".cards",
    "\"main\".\"cards\"",
    "[main].[cards]",
    "main . cards",
    "cards \"c\"",
    "cards as \"a b\"",
    "cards as [c]",
    "(cards)",
    "(select * from cards)",
    "(select * from cards) s",
    "(select * from cards) as \"s\"",
    "((select * from cards)) s",
    "cards indexed by sqlite_autoindex_cards_1",
    "cards not indexed",
];

/// Ways of spelling it where a write names its table.
const TARGETS: &[&str] = &["cards", "main.cards", "CARDS", "\"cards\"", "`cards`", "[cards]", "\"main\".\"cards\"", "cards as c", "main . cards"];

/// Ways of dressing a statement up that leave what it says alone.
fn dressed(sql: &str) -> Vec<String> {
    let quoted = sql.contains(['"', '`', '[', '\'']);
    let mut out = vec![
        sql.to_string(),
        format!("{sql};"),
        format!("  {sql}  "),
        sql.replace(' ', "\n"),
        sql.replace(' ', "\t"),
        sql.replace(' ', "  "),
        sql.replace(' ', " /* c */ "),
        sql.replace(' ', "/**/"),
        sql.replace(" from ", " from -- c\n "),
        sql.replace(" from ", "\nfrom\n"),
        sql.replace("(select", "( select").replace("from (", "from("),
        format!("/* select * from orders */ {sql}"),
        format!("{sql} -- from orders"),
        format!("{sql} /* ' */"),
        format!("{sql} -- '"),
        format!("{sql} -- \""),
        sql.replace(" from ", " FROM ").replace("select", "SELECT").replace(" join ", " JOIN "),
        sql.replace(" where ", " where 'a from orders' = 'x''y' and "),
        sql.replace(" where ", " where \"note\" = 'from' and "),
    ];
    // Upper case is the same statement where nothing is quoted (a quoted name keeps its case).
    if !quoted {
        out.push(sql.to_uppercase());
    }
    out.sort();
    out.dedup();
    out
}

/// Statements written to make two readers disagree: each reads or writes `cards` under SQLite,
/// behind something another reader takes for a name, a string or a comment.
const TRICKS: &[(&str, bool)] = &[
    // SQLite reads `[…]` as a name; others read brackets as code, quotes inside them included.
    ("select 1 as [\"], id from cards -- \"]", false),
    ("select 1 as ['], id from cards -- ']", false),
    ("select 1 as [`], id from cards -- `]", false),
    ("select 1 as [a -- ], id from cards", false),
    ("select 1 as [a /* ], id from cards -- */ x]", false),
    ("select [id] from orders union all select [id] from cards", false),
    ("select 1 as [\"] from orders union all select id from cards -- \"]", false),
    ("select 1 as [x'] from orders, cards where '1' = '1'", false),
    // A quote doubled inside a quoted name or a string is one character of it.
    ("select 1 as \"a\"\" from orders --\", id from cards", false),
    ("select 'it''s', id from cards", false),
    ("select 1 as `a`` from orders`, id from cards", false),
    ("select 'a' as x, id from cards where 'b''--' = 'c'", false),
    // Names called as functions.
    ("select \"upper\"(pan) from cards", false),
    ("select `upper`(pan) from cards", false),
    ("select [upper](pan) from cards", false),
    ("select \"load_extension\"('x') from orders", false),
    ("select `randomblob`(4) from orders", false),
    ("select main.upper(note) from orders", false),
    ("select random() from orders", false),
    ("select * from orders where id = last_insert_rowid()", false),
    ("select sqlite_version()", false),
    ("select datetime('now') from orders", false),
    ("select * from orders where note glob (select pan from cards)", false),
    // Tables that are functions, and the schema's own.
    ("select * from pragma_table_info('cards')", false),
    ("select * from \"pragma_table_info\"('cards')", false),
    ("select * from json_each('[1]')", false),
    ("select * from orders, json_each((select pan from cards))", false),
    ("select sql from sqlite_master", false),
    ("select sql from main.sqlite_schema", false),
    // A read that isn't one.
    ("select * from orders; delete from cards", false),
    ("delete from cards", false),
    ("insert into cards (id) values (1)", false),
    ("pragma table_info(cards)", false),
    ("select 1; pragma writable_schema = 1", false),
    ("attach 'x.db' as other", false),
    ("vacuum", false),
    // A write that names another table than it writes, or more.
    ("insert into audit (what) values (1); delete from cards", true),
    ("update orders set note = 1; delete from cards", true),
    ("create table x as select * from cards", true),
    ("drop table cards", true),
    ("alter table cards rename to x", true),
    ("create trigger t after insert on audit begin delete from cards; end", true),
    ("with r as (select 1) delete from cards", true),
    ("with r as (select id from cards) insert into audit (what) select id from r", true),
    ("insert into audit (what) values (1) returning (select max(id) from cards)", true),
    ("update orders set note = 1 returning (select max(id) from cards)", true),
    ("delete from [cards]", true),
    ("update main.cards set pan = 1", true),
    ("insert into audit (what) select 1 where exists (select 1 from cards) -- '", true),
    ("delete from orders as cards where id in (select id from cards)", true),
    // Numbers, odd spaces, and what isn't SQL.
    ("select 1from cards", false),
    ("select 1.e1from cards", false),
    ("select 0x10from cards", false),
    ("select * from\u{b}cards", false),
    ("select * from\u{c}cards", false),
    ("select * from\u{a0}cards", false),
    ("select * from\u{0}cards", false),
    ("select x'00' from cards", false),
    ("select * from cards where id = ?1", false),
    ("select * from cards where id = :id", false),
    ("select * from cards where id = @id", false),
];

/// What Postgres or MySQL reads where SQLite reads otherwise, or not at all: the statement,
/// whether it is sent as a write, the tables that engine reads and writes, and whether the scan is
/// to take it (then it names exactly those) or may refuse it (it names them all, or refuses). A
/// statement that calls a function no scan can vet has `CALLS` among its reads, so only refusing
/// holds.
const CALLS: &str = "(a function the scan can't vet)";
type Elsewhere = (&'static str, &'static str, bool, &'static [&'static str], &'static [&'static str], bool);
const TAKEN: bool = true;
const MAY_REFUSE: bool = false;
const ELSEWHERE: &[Elsewhere] = &[
    ("Postgres: a schema's table keeps its dot", "select * from a, b as bb, public.c where a.x = bb.y", false, &["a", "b", "public.c"], &[], TAKEN),
    ("Postgres: an alias names its columns", "select * from (values (1), (2)) as v(n)", false, &[], &[], TAKEN),
    ("Postgres: a with sees only the names before it, so a later one is a table there", "with a as (select * from b), b as (select 1 as x) select * from a", false, &["b"], &[], TAKEN),
    ("Postgres, MySQL: a from inside a call names no table", "select extract(year from placed) from orders", false, &["orders"], &[], TAKEN),
    ("Postgres", "select trim(both 'x' from name), substring(name from 2 for 3) from people", false, &["people"], &[], TAKEN),
    ("Postgres: a cast with a size", "select cast(total as decimal(10, 2)), total::numeric(10,2) from orders", false, &["orders"], &[], TAKEN),
    ("Postgres: brackets are code", "select tags[1], tags[$1] from orders", false, &["orders"], &[], TAKEN),
    ("Postgres", "insert into orders (tags) values (array[(select password from users limit 1)])", true, &["users"], &["orders"], TAKEN),
    ("Postgres, MySQL 8: `table x` is `select * from x`", "select id from orders union all table cards", false, &["orders", "cards"], &[], MAY_REFUSE),
    ("Postgres, MySQL 8", "select * from orders where id in (table cards)", false, &["orders", "cards"], &[], MAY_REFUSE),
    ("Postgres", "insert into audit table cards", true, &["cards"], &["audit"], MAY_REFUSE),
    ("Postgres: a quoted name is called like another", "select \"query_to_xml\"($1, true, false, '')::text from orders", false, &["orders", CALLS], &[], MAY_REFUSE),
    ("Postgres", "select \"pg_read_file\"('/etc/passwd') from orders", false, &["orders", CALLS], &[], MAY_REFUSE),
    ("Postgres: a function of a schema by a known name", "select public.lower(note) from orders", false, &["orders", CALLS], &[], MAY_REFUSE),
    ("Postgres", "select pg_catalog.\"pg_sleep\"(1) from orders", false, &["orders", CALLS], &[], MAY_REFUSE),
    ("MySQL", "select shop.upper(note) from orders", false, &["orders", CALLS], &[], MAY_REFUSE),
    ("Postgres: U& writes a name by its code points", "select pan from U&\"\\0063ards\"", false, &["cards"], &[], MAY_REFUSE),
    ("Postgres", "select pan from u&\"c\\0061rds\" where id = $1", false, &["cards"], &[], MAY_REFUSE),
    ("Postgres before 15: a number runs into the word after it", "select 1from cards", false, &["cards"], &[], MAY_REFUSE),
    ("Postgres before 15", "select id from orders where 1=1and exists (select 1from cards)", false, &["orders", "cards"], &[], MAY_REFUSE),
    ("MySQL: an update of several tables writes any of them", "update orders, payments set payments.amount = 0 where payments.order_id = orders.id", true, &["orders", "payments"], &["orders", "payments"], MAY_REFUSE),
    ("MySQL", "update orders join payments on payments.order_id = orders.id set payments.amount = 0", true, &["orders", "payments"], &["orders", "payments"], MAY_REFUSE),
    ("MySQL", "update orders o, cards c set c.pan = '0' where o.id = 1", true, &["orders", "cards"], &["orders", "cards"], MAY_REFUSE),
    ("MySQL: a delete from several tables", "delete from orders, payments using orders join payments on payments.order_id = orders.id", true, &["orders", "payments"], &["orders", "payments"], MAY_REFUSE),
    ("MySQL: a word before the table says how, not what", "update ignore cards set pan = $1", true, &["cards"], &["cards"], MAY_REFUSE),
    ("MySQL", "update low_priority cards set pan = $1", true, &["cards"], &["cards"], MAY_REFUSE),
    ("MySQL", "update low_priority ignore cards set pan = $1", true, &["cards"], &["cards"], MAY_REFUSE),
    ("MySQL", "delete quick from cards", true, &["cards"], &["cards"], MAY_REFUSE),
    ("MySQL", "delete ignore from cards where id = $1", true, &["cards"], &["cards"], MAY_REFUSE),
    ("MySQL", "insert low_priority into cards (id) values ($1)", true, &[], &["cards"], MAY_REFUSE),
    ("MySQL", "insert delayed ignore into cards (id) values ($1)", true, &[], &["cards"], MAY_REFUSE),
    ("MySQL: straight_join is a join", "select * from orders straight_join cards on cards.id = orders.id", false, &["orders", "cards"], &[], MAY_REFUSE),
    ("MySQL", "select * from orders o straight_join cards", false, &["orders", "cards"], &[], MAY_REFUSE),
    ("Postgres: only, and a delete's using", "select * from only cards", false, &["cards"], &[], TAKEN),
    ("Postgres", "delete from orders using cards where cards.id = orders.id", true, &["orders", "cards"], &["orders"], TAKEN),
    ("Postgres", "update orders set note = cards.pan from cards where cards.id = orders.id", true, &["orders", "cards"], &["orders"], TAKEN),
    ("Postgres: lateral", "select * from orders, lateral (select * from cards where cards.id = orders.id) c", false, &["orders", "cards"], &[], TAKEN),
    ("Postgres: a subquery in brackets", "select array[(select pan from cards limit 1)] from orders", false, &["orders", "cards"], &[], TAKEN),
    ("Postgres", "select id from orders where tags[(select count(*) from cards)] is not null", false, &["orders", "cards"], &[], TAKEN),
];

fn fill(template: &str, hole: &str, with: &str) -> String {
    template.replace(hole, with)
}

/// **Guards:** a statement the scan takes touches no table the scan didn't name and calls no
/// function the scan didn't vet, and a plain statement is taken and named exactly. What the scan
/// names is what a flow's manifest shows (`DbRead("shop/orders")`), what a person approves, and
/// what the host checks the statement against again as it is sent (holon's databases.rs runs the
/// same scan).
///
/// **Why:** the scan is a tokenizer and a walk over a small part of SQL, written by hand, and an
/// engine reads the same text with a grammar of its own. Wherever the two read a text differently
/// (a quote, a bracket, a comment, a keyword), a table can sit where the scan doesn't look. No list
/// of cases written by hand finds those; the engine has to be asked, over every combination.
///
/// **If it fails:** a flow whose approval says "read shop/orders" reads or writes another table of
/// that database (card numbers, say), or runs a function of the engine's that does more than
/// compute. Nothing after it catches that: the grant is checked with this same scan. If it fails on
/// exactness instead, an ordinary statement is refused or asks for a table it doesn't touch.
#[test]
fn a_statement_the_scan_takes_touches_and_calls_nothing_the_scan_didnt_name() {
    let db = engine();
    let mut missed: Vec<String> = Vec::new();
    let (mut asked, mut taken, mut compared) = (0usize, 0usize, 0usize);
    let mut check = |sql: &str, write: bool, missed: &mut Vec<String>| {
        asked += 1;
        // What SQLite doesn't take never runs; what the scan refuses is never sent.
        let Some(says) = sqlite_says(&db, sql) else { return };
        match holds(sql, write, &says) {
            Ok(true) => {
                taken += 1;
                compared += 1;
            }
            Ok(false) => compared += 1,
            Err(why) => missed.push(why),
        }
    };

    // Every place a table can be named, every spelling of it, every dressing.
    for template in READS {
        for spelling in SPELLINGS {
            for sql in dressed(&fill(template, "{T}", spelling)) {
                check(&sql, false, &mut missed);
            }
        }
    }
    for template in WRITES {
        let (hole, spellings) = if template.contains("{W}") { ("{W}", TARGETS) } else { ("{T}", SPELLINGS) };
        for spelling in spellings {
            for sql in dressed(&fill(template, hole, spelling)) {
                check(&sql, true, &mut missed);
                // Sent as a read, a write is refused or seen as one.
                check(&sql, false, &mut missed);
            }
        }
    }
    for (sql, write) in TRICKS {
        for sql in dressed(sql) {
            check(&sql, *write, &mut missed);
        }
    }
    assert!(missed.is_empty(), "{} statements touch what the scan didn't name:\n{}", missed.len(), missed.join("\n"));
    // The generator makes what SQLite takes, and the scan takes most of it: neither is vacuous.
    assert!(asked > 15_000 && compared > 8_000, "{asked} statements, {compared} SQLite takes");
    assert!(taken * 2 > compared, "the scan takes {taken} of the {compared} statements SQLite takes");

    // A plain statement is taken, and named exactly: no table more than SQLite says.
    let mut inexact = Vec::new();
    let plain_spellings = &SPELLINGS[..3];
    let mut plainly: Vec<(String, bool)> = PLAIN.iter().map(|(sql, write)| (sql.to_string(), *write)).collect();
    for (sql, _) in &plainly {
        assert!(sqlite_says(&db, sql).is_some(), "SQLite takes every plain statement: {sql:?}");
    }
    for (templates, write) in [(READS, false), (WRITES, true)] {
        for template in templates {
            let (hole, spellings): (&str, &[&str]) = if template.contains("{W}") { ("{W}", &TARGETS[..1]) } else { ("{T}", plain_spellings) };
            plainly.extend(spellings.iter().map(|spelling| (fill(template, hole, spelling), write)));
        }
    }
    for (sql, write) in plainly {
        let Some(says) = sqlite_says(&db, &sql) else { continue };
        let scanned = match tables(&sql, write) {
            Ok(scanned) => scanned,
            Err(why) => {
                inexact.push(format!("{sql:?}: refused, {why}"));
                continue;
            }
        };
        let touched: BTreeSet<String> = scanned.reads.union(&scanned.writes).map(|t| plain(t)).collect();
        let truly: BTreeSet<String> = says.reads.union(&says.writes).cloned().collect();
        let writes: BTreeSet<String> = scanned.writes.iter().map(|t| plain(t)).collect();
        if touched != truly || writes != says.writes {
            inexact.push(format!("{sql:?}: the scan touches {touched:?} writes {writes:?}, SQLite reads {:?} writes {:?}", says.reads, says.writes));
        }
    }
    assert!(inexact.is_empty(), "{} plain statements aren't named exactly:\n{}", inexact.len(), inexact.join("\n"));
}

/// **Guards:** the same statement as the test above, for what only Postgres or MySQL reads: a
/// statement the scan takes touches no table the scan didn't name, and an ordinary one is taken
/// and named exactly.
///
/// **Why:** one scan serves three engines, and SQLite can't speak for a keyword it doesn't have
/// (`table x`, `straight_join`, an update of several tables, `U&"…"`). Each row was read out of
/// that engine's grammar by hand; there is no engine here to ask.
///
/// **If it fails:** as above, on that engine: a flow touches a table its approval doesn't show.
/// A row that is to be taken and isn't means an ordinary statement stopped compiling.
#[test]
fn what_another_engine_reads_in_a_statement_the_scan_names_or_refuses() {
    let (mut missed, mut inexact) = (Vec::new(), Vec::new());
    for (engine, sql, write, reads, writes, taken) in ELSEWHERE {
        let truly: BTreeSet<String> = reads.iter().chain(writes.iter()).map(|t| t.to_string()).collect();
        let truly_written: BTreeSet<String> = writes.iter().map(|t| t.to_string()).collect();
        for dressing in dressed(sql) {
            // As it is written, a statement to take is taken. Dressed up it may be refused for its
            // dress (a comment inside a bracket is), and is named exactly when it isn't.
            let scanned = match tables(&dressing, *write) {
                Ok(scanned) => scanned,
                Err(why) if *taken && dressing == *sql => {
                    inexact.push(format!("{engine}\n    {sql:?}: refused, {why}"));
                    continue;
                }
                Err(_) => continue,
            };
            let sql = dressing;
            let touched: BTreeSet<String> = scanned.reads.union(&scanned.writes).map(|t| plain(t)).collect();
            let written: BTreeSet<String> = scanned.writes.iter().map(|t| plain(t)).collect();
            let unread: Vec<&String> = truly.difference(&touched).collect();
            let unwritten: Vec<&String> = truly_written.difference(&written).collect();
            if !unread.is_empty() || !unwritten.is_empty() {
                missed.push(format!("{engine}\n    {sql:?}\n    the scan: reads {:?} writes {:?}; unread {unread:?}, unwritten {unwritten:?}", scanned.reads, scanned.writes));
            } else if *taken && (touched != truly || written != truly_written) {
                inexact.push(format!("{engine}\n    {sql:?}: the scan touches {touched:?} writes {written:?}, the engine {truly:?} writes {truly_written:?}"));
            }
        }
    }
    assert!(missed.is_empty(), "{} statements touch what the scan didn't name:\n{}", missed.len(), missed.join("\n"));
    assert!(inexact.is_empty(), "{} ordinary statements aren't taken, or not named exactly:\n{}", inexact.len(), inexact.join("\n"));
}
