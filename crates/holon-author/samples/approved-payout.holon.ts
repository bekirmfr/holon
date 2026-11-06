// Approved payout: a waiting run checks what it read.
//
// Asks for a payout from an account. The route answers with an approval link (202) and the
// account's balance, and starts the payout's run, an action, which waits for the approval, a day at
// most, holding nothing on the account meanwhile: a deposit goes ahead while it waits. Once
// approved, the run reads the balance as it is now, sends the payout (to httpbin.org, standing in
// for a bank), and writes the new balance and a record of the payout. At its commit it checks what
// it read: if the balance was changed outside Holon while the payout was in flight, it runs again
// from the approval on the balance as it now is, the payout and the approval answered from its
// record, not sent or asked again. No deposit is written over, and nothing is paid twice.
//
// Before it works, an admin binds the database, with its tables and one account holding 100:
//   holon database add bank ~/holon-bank.db --schema "create table if not exists accounts
//     (id integer primary key, balance integer not null);
//     insert or ignore into accounts values (1, 100);
//     create table if not exists payouts (account integer not null, amount integer not null,
//     status integer)"
//
// Try it: deploy it, then
//   curl -X POST -d '{"amount":30}' http://127.0.0.1:8153/payout/1    an approve link
//   curl -X POST -d '{"amount":50}' http://127.0.0.1:8153/deposit/1   150, while the payout waits
//   curl -X POST -d '{"by":"ops"}' http://127.0.0.1:8153<the link>    pays it
//   curl http://127.0.0.1:8153/accounts/1                            120: the deposit kept
import { action, event, flow, route, status } from "holon"
import bank from "database:bank"
import httpbin from "host:httpbin.org"

flow("approved-payout", { port: 8153 })

// Starts the payout's run and answers its approval link, with the balance as it is now.
route.post("/payout/:account", (req) => {
  const account = bank.one("select id, balance from accounts where id = $1", req.params.account)
  const started = payout({ account: account.id, amount: req.json.amount })
  return status(202, { approve: `/approve/${started.waiting.approved}`, balance: account.balance })
})

// The payout's run: waits for its approval, then pays from the balance as it is then, and
// records it.
export const payout = action(async (asked) => {
  const account = bank.one("select id, balance from accounts where id = $1", asked.account)
  // The run waits here until the link is opened, for a day at most.
  const approved = await event("approved", { expires: "1d" })
  const sent = await httpbin.post("/anything/payouts", {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      account: account.id,
      amount: asked.amount,
      by: JSON.parse(approved.body).by,
    }),
  })
  bank.exec(
    "update accounts set balance = $1 where id = $2",
    account.balance - asked.amount,
    account.id,
    { expect: 1 },
  )
  bank.exec(
    "insert into payouts (account, amount, status) values ($1, $2, $3)",
    account.id,
    asked.amount,
    sent.status,
  )
})

// Opening the link approves the waiting payout; its body's `by` says who approved.
route.post("/approve/:token", () => "approved", { resumes: "approved" })

// Adds to an account's balance and answers the new one.
route.post("/deposit/:account", (req) => {
  const account = bank.one("select id, balance from accounts where id = $1", req.params.account)
  bank.exec(
    "update accounts set balance = $1 where id = $2",
    account.balance + req.json.amount,
    account.id,
    { expect: 1 },
  )
  return { balance: account.balance + req.json.amount }
})

// An account's balance.
route.get("/accounts/:account", (req) =>
  bank.one("select balance from accounts where id = $1", req.params.account),
)
