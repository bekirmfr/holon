// Ticket numbers: a database held for whole runs.
//
// Hands out numbered tickets from one counter row: each request reads the last number, writes the
// next and records who has it. The text is an ordinary database flow; what it shows is the admin's
// setting on the database. Bound with --lock run, each run that may write it holds it from before
// it starts until it commits, so a burst of requests takes numbers one after the other, every run
// committing on its first try. Readers of the list hold it shared, side by side. The cost: the
// database's writers go one at a time, across every deployment bound to it (and every server, on
// Postgres or MySQL), and one that would wait past its claim_wait is turned away (503).
//
// Before it works, an admin binds the database, held for whole runs, with its tables:
//   holon database add tickets ~/holon-tickets.db --lock run --schema "create table if not exists
//     counter (id integer primary key, n integer not null);
//     insert or ignore into counter values (1, 0);
//     create table if not exists issued (n integer primary key, holder text not null)"
//
// Try it: deploy it, then take twenty at once:
//   for i in $(seq 20); do
//     curl -s -X POST -d '{"name":"p'$i'"}' http://127.0.0.1:8152/ticket &
//   done; wait
//   curl http://127.0.0.1:8152/tickets                  1 to 20, one each
// and the deployment's log shows no run ran again.
import { flow, route, status } from "holon"
import tickets from "database:tickets"

flow("ticket-numbers", { port: 8152 })

// Takes the next number, records who holds it, and answers it.
route.post("/ticket", (req) => {
  const last = tickets.one("select n from counter where id = $1", 1)
  tickets.exec("update counter set n = $1 where id = $2", last.n + 1, 1, { expect: 1 })
  tickets.exec("insert into issued (n, holder) values ($1, $2)", last.n + 1, req.json.name)
  return status(201, { ticket: last.n + 1 })
})

// Every ticket handed out, by number.
route.get("/tickets", () => tickets.query("select n, holder from issued order by n"))
