// Approved payout: an agent classifies it, a person reviews it, another approves it, and the
// waiting run checks what it read.
//
// Asks for a payout from an account. The route answers at once (202) with the account's balance,
// and starts the payout's run, an action, which waits at an approval in three steps, in turn, a
// day at most: an agent classifies the payout, with a note the run is given (its label, its
// confidence), a person reviews it, and another approves it. Each step is taken by whoever is
// bound to its role on the flow, of its kind, each a different principal, and never by whoever
// started the run; one decline, with a reason, ends it. They decide in the editor's Approvals
// (Runs waiting) or with holon decide, the agent with its own token or holon mcp's decide tool.
// The run holds nothing on the account meanwhile: a deposit goes ahead while it waits. Approved,
// it reads the balance as it is now, sends the payout (to httpbin.org, standing in for a bank)
// with who approved it and the classifier's note, and writes the new balance and a record of the
// payout. Every decision is kept with the binding that let it and the access version it was read
// in, in the run's log and its export, for an auditor to check against holon access show. At its
// commit the run checks what it read: if the balance was changed outside Holon while the payout
// was in flight, it runs again from the approval on the balance as it now is, the payout and the
// approval answered from its record, not sent or asked again. No deposit is written over, and
// nothing is paid twice.
//
// It is tagged payments: whoever is bound on the tag acts on it, as on every flow deployed with
// the tag, and on no other. A version that adds or drops a tag is approved like anything a flow
// asks for, and the review says who the tag lets in.
//
// Before it works, an admin says who takes each step: the three roles, an agent and two people
// (each person is printed the link they join with), each bound on the tag:
//   holon roles define classifier --allow run.decide --kinds agent
//   holon roles define reviewer --allow run.decide
//   holon roles define approver --allow run.decide
//   holon people add scout --agent
//   holon people token scout                                         its token, printed once
//   holon people add bob --person --role viewer
//   holon people add cy --person --role viewer
//   holon bind classifier --to scout --tag payments
//   holon bind reviewer --to bob --tag payments
//   holon bind approver --to cy --tag payments
// And binds the database, with its tables and one account holding 100:
//   holon database add bank ~/holon-bank.db --schema "create table if not exists accounts
//     (id integer primary key, balance integer not null);
//     insert or ignore into accounts values (1, 100);
//     create table if not exists payouts (account integer not null, amount integer not null,
//     status integer)"
//
// Try it: deploy it, then
//   curl -X POST -d '{"amount":30}' http://127.0.0.1:8153/payout/1    waits for the classifier
//   curl -X POST -d '{"amount":50}' http://127.0.0.1:8153/deposit/1   150, while the payout waits
//   holon decide approved-payout                                      the run, its steps, whose
// scout classifies it, with its token in HOLON_TOKEN, and a note:
//   holon decide approved-payout <run> payout --approve --note '{"label":"invoice","confidence":0.93}'
// then bob reviews it and cy approves it, each signed in to the editor, in Approvals: each only
// their own step, and in turn. It pays it:
//   curl http://127.0.0.1:8153/accounts/1                            120: the deposit kept
import { action, approval, flow, route, status } from "holon"
import bank from "database:bank"
import httpbin from "host:httpbin.org"

flow("approved-payout", { port: 8153, tags: ["payments"] })

// Starts the payout's run and answers at once, with the balance as it is now.
route.post("/payout/:account", (req) => {
  const account = bank.one("select id, balance from accounts where id = $1", req.params.account)
  payout({ account: account.id, amount: req.json.amount })
  return status(202, {
    waiting: "classifier, then reviewer, then approver",
    balance: account.balance,
  })
})

// The payout's run: waits for its three steps, then pays from the balance as it is then, and
// records it.
export const payout = action(async (asked) => {
  const account = bank.one("select id, balance from accounts where id = $1", asked.account)
  // The run waits here for each step in turn, a day at most: an agent bound to classifier, then
  // a person bound to reviewer, then one bound to approver.
  const decision = await approval("payout", {
    steps: [{ role: "classifier", kind: "agent" }, { role: "reviewer" }, { role: "approver" }],
    expires: "1d",
  })
  // Declined (with why), or nobody decided in time: nothing is sent or written.
  if (!decision.approved) {
    return { declined: decision.reason ?? "expired" }
  }
  const sent = await httpbin.post("/anything/payouts", {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      account: account.id,
      amount: asked.amount,
      by: decision.by,
      class: decision.decisions[0].note,
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
