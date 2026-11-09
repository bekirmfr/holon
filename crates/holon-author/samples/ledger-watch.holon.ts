// Ledger watch: each account's balance, kept as a ledger's entries are written.
//
// A ledger in Postgres, which another service writes: each entry its account's next (seq 1, 2,
// 3, …), and a trigger that sends it on the channel ledger_entries (pg_notify) as it commits. The
// binding listens on the channel (--database ledger --channel ledger_entries), so each entry
// written starts a run, e.data the entry. The handler adds its amount to its account's balance.
// Postgres keeps no notification for a session that isn't listening, so an entry written while
// the connection was lost (the database restarted, the server stopped) is never sent: when an
// entry's seq is past the one after the account's last, the handler reads the entries between
// from the ledger and adds them too, so the balance stays exact. An entry added already changes
// nothing. The balance and the last seq are written by one run, so they move together.
//
// Before it works, an admin binds the database, read-only, its table and trigger made by
// ledger.sql, and the feed:
//   HOLON_SECRET_LEDGER_URL=postgres://holon:<password>@127.0.0.1:5432/ledger
//   holon database add ledger env:HOLON_SECRET_LEDGER_URL --read-only --schema-file ledger.sql
//   holon feed add entries --database ledger --channel ledger_entries
// where ledger.sql is
//   create table if not exists entries (id bigint generated always as identity primary key,
//     account text not null, seq integer not null, amount numeric(18,2) not null, memo text,
//     unique (account, seq));
//   create or replace function entries_notify() returns trigger language plpgsql as $$ begin
//     perform pg_notify('ledger_entries', json_build_object('id', new.id, 'account',
//       new.account, 'seq', new.seq, 'amount', new.amount::text, 'memo', new.memo)::text);
//     return null; end $$;
//   create or replace trigger entries_notify after insert on entries for each row
//     execute function entries_notify();
//
// Try it: deploy it, then write the ledger (psql, or the service that keeps it):
//   insert into entries (account, seq, amount) values ('acct_1', 1, 120.00), ('acct_1', 2, -20.50);
//   curl http://127.0.0.1:8195/balances      acct_1: 99.50, from 2 entries
import { decimal_sum, flow, on, route } from "holon"
import balances from "collection:balances"
import ledger from "database:ledger"
import entries from "feed:entries"

flow("ledger-watch", { port: 8195 })

// Each entry, {id, account, seq, amount, memo}, added to its account's balance, with the entries
// before it that weren't heard, read from the ledger.
on.feed(entries, (e) => {
  const entry = e.data
  const was = balances[entry.account] ?? { balance: "0", entries: 0, read: 0, last: 0 }
  const unheard =
    entry.seq > was.last + 1
      ? ledger.query(
          "select amount from entries where account = $1 and seq > $2 and seq < $3",
          entry.account,
          was.last,
          entry.seq,
        )
      : []
  const amounts = unheard.map((u) => u.amount)
  if (entry.seq > was.last)
    balances[entry.account] = {
      balance: decimal_sum([was.balance, entry.amount, ...amounts]),
      entries: was.entries + 1 + amounts.length,
      read: was.read + amounts.length,
      last: entry.seq,
    }
})

// Each account's balance: its entries, those read from the ledger, and its last seq.
route.get("/balances", () => balances.list())
