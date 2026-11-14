// Approved payout: an agent classifies it, a person reviews it, another approves it, and the
// waiting run checks what it read.
//
// Asks for a payout from an account. The route answers at once (202) with the account's balance,
// and starts the payout's run, an action, which waits at an approval in three steps, in turn, a
// day at most: an agent classifies the payout, with a note the run is given (its label, its
// confidence), a person reviews it, and another approves it. Each step is taken by whoever is
// bound to its role on the flow, of its kind, each a different principal, and never by whoever
// started the run; one decline, with a reason, ends it. Each sees what they decide: the account,
// the amount and the balance the run read (its shows). They decide in the editor's Approvals
// (Runs waiting draws the three steps as a chain: who took each, the classifier's note, what is
// open) or with holon decide: a person on another machine signs in first with holon login, the
// agent uses its own token or holon mcp's decide tool.
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
// Before it works, an admin says who takes each step: the three roles, an agent, a person (who is
// printed the link they join with) and you, each bound on the tag. Deployed before they are, its
// review marks its approval as one no one here can decide yet (classifier: no agent holds it
// here), and the editor asks again before deploying it. Bound so, with you its one approver, it
// still notes that a run you start yourself can't be decided (you take no step of it), without
// asking: the runs its route starts can be. On the server's machine holon signs in as you, the
// account that set the server up:
//   holon roles define classifier --allow run.decide --kinds agent
//   holon roles define reviewer --allow run.decide
//   holon roles define approver --allow run.decide
//   holon people add scout --agent
//   holon people token scout                                         its token, printed once
//   holon people add bob --person --role viewer
//   holon bind classifier --to scout --tag payments
//   holon bind reviewer --to bob --tag payments
//   holon bind approver --to <you> --tag payments
// Or in the editor's People and access: Roles defines the three (the classifier held by an agent
// alone), Agents and tokens adds scout and makes its token, People adds bob, and the bind form,
// on Bindings or on each one's page, binds each role to its principal on the flows with a tag,
// payments (until: empty, for good; why: History keeps it). Each one's page then says what they
// decide.
// With access changes governed, a binding that gives someone more waits for another admin. Add
// one first (holon people add eve --person --role admin), then holon access approvals --changes
// 1: binding reviewer to bob now isn't made at once. The form says it waits for 1 person other
// than you to approve it (access change 1), and History lists it. Eve approves it in the editor's
// Approvals › Access changes, which says what it gives and who could act as bob (you made bob's
// invite link, so you don't approve it), or with holon access approve 1, and it is made. Binding
// approver to you is made at once: it gives an admin nothing more.
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
//   holon decide approved-payout                                      the run, its steps, whose,
//                                                                     and what it shows
// scout classifies it, with its token in HOLON_TOKEN, and a note:
//   holon decide approved-payout <run> payout --approve --note '{"label":"invoice","confidence":0.93}'
// then bob reviews it, in the editor's Approvals, or from another machine (holon login --server
// <URL>, then the same holon decide), and you approve it on the server's machine, with
// HOLON_TOKEN unset: each only their own step, and in turn. It pays it:
//   holon decide approved-payout <run> payout --approve
//   curl http://127.0.0.1:8153/accounts/1                            120: the deposit kept
// Signed in as bob, the flow's page has each button that would change it disabled, saying why in
// the server's words (no role bob holds on "approved-payout" holds flow.lifecycle): he reviews its
// payouts; bound editor on the flow, his page follows within seconds.
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
  // a person bound to reviewer, then one bound to approver. Each sees the payout it decides.
  const decision = await approval("payout", {
    steps: [{ role: "classifier", kind: "agent" }, { role: "reviewer" }, { role: "approver" }],
    expires: "1d",
    shows: { account: account.id, amount: asked.amount, balance: account.balance },
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
