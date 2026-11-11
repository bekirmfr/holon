// Approved payout: two people approve it, and the waiting run checks what it read.
//
// Asks for a payout from an account. The route answers at once (202) with the account's balance,
// and starts the payout's run, an action, which waits for two different people to approve it, a
// day at most, holding nothing on the account meanwhile: a deposit goes ahead while it waits.
// People signed in to Holon decide it, in the editor's Approvals (Runs waiting) or with holon
// decide; each counts once, and one decline, with a reason, ends it. Approved, the run reads the
// balance as it is now, sends the payout (to httpbin.org, standing in for a bank) with who
// approved it, and writes the new balance and a record of the payout. At its commit it checks
// what it read: if the balance was changed outside Holon while the payout was in flight, it runs
// again from the approval on the balance as it now is, the payout and the approval answered from
// its record, not sent or asked again. No deposit is written over, and nothing is paid twice.
//
// It is tagged payments: whoever is bound on the tag acts on it, as on every flow deployed with
// the tag, and on no other. A version that adds or drops a tag is approved like anything a flow
// asks for, and the review says who the tag lets in.
//
// Where it goes next (docs/L4s/access-and-approvals-L4.md, §8 and §13): an approval in steps,
// an agent classifying, an editor reviewing, an admin approving, each a different principal.
//
// Before it works, an admin binds the database, with its tables and one account holding 100:
//   holon database add bank ~/holon-bank.db --schema "create table if not exists accounts
//     (id integer primary key, balance integer not null);
//     insert or ignore into accounts values (1, 100);
//     create table if not exists payouts (account integer not null, amount integer not null,
//     status integer)"
//
// Try it: deploy it, then
//   curl -X POST -d '{"amount":30}' http://127.0.0.1:8153/payout/1    waits for two people
//   curl -X POST -d '{"amount":50}' http://127.0.0.1:8153/deposit/1   150, while the payout waits
//   holon decide approved-payout                                      the run, and who approved
// Then two people approve it: you (holon decide approved-payout <run> payout --approve), and
// someone you invited and bound on the tag, in the editor's Approvals:
//   holon people add bob --person
//   holon bind editor --to bob --tag payments                         bob acts on payments' flows
// It pays it:
//   curl http://127.0.0.1:8153/accounts/1                            120: the deposit kept
import { action, approval, flow, route, status } from "holon"
import bank from "database:bank"
import httpbin from "host:httpbin.org"

flow("approved-payout", { port: 8153, tags: ["payments"] })

// Starts the payout's run and answers at once, with the balance as it is now.
route.post("/payout/:account", (req) => {
  const account = bank.one("select id, balance from accounts where id = $1", req.params.account)
  payout({ account: account.id, amount: req.json.amount })
  return status(202, { waiting: "two people's approval", balance: account.balance })
})

// The payout's run: waits for two people to approve it, then pays from the balance as it is then,
// and records it.
export const payout = action(async (asked) => {
  const account = bank.one("select id, balance from accounts where id = $1", asked.account)
  // The run waits here until two different people approve it, a day at most.
  const decision = await approval("payout", { people: 2, expires: "1d" })
  // Declined (with why), or nobody decided in time: nothing is sent or written.
  if (!decision.approved) {
    return { declined: decision.reason ?? "expired" }
  }
  const sent = await httpbin.post("/anything/payouts", {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ account: account.id, amount: asked.amount, by: decision.by }),
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
  return { paid: asked.amount, by: decision.by }
})

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
