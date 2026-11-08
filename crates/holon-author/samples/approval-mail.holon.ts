// Approval mail: a second person approves each payout from a mail.
//
// Asking for a payout starts its run, which waits for an approval, a day at most, and mails
// approvals@acme.com a link to approve it, the payout attached as a CSV. The link opens a page
// whose button approves: a mail scanner that opens every link approves nothing. The page isn't
// recorded, so the token in its link never reaches the log. Once approved, the payout is recorded,
// and whoever asked for it is mailed that it was: an address from the request, which the grant
// can't name, so the binding's --to holds it (*@acme.com here). Mail goes once its run is over and
// its writes have landed, from the binding's sender, and is tried again for a day when the server
// can't take it.
//
// Before it works, an admin binds the mail, its server's URL and login in a secret source:
//   HOLON_SECRET_OPS_SMTP=smtps://user:password@smtp.example.com   (the server's environment)
//   holon mail add ops --smtp env:HOLON_SECRET_OPS_SMTP --from "Treasury <treasury@acme.com>"
//     --to "*@acme.com"
//
// Try it: deploy it, then
//   curl -X POST -d '{"id":"p1","amount":5000,"iban":"DE89370400440532013000","by":"sam@acme.com"}'
//     http://127.0.0.1:8186/payouts
//   open the link approvals@acme.com was mailed, and press Approve: sam@acme.com is mailed
//   curl http://127.0.0.1:8186/payouts                    p1, approved
import { action, event, flow, respond, route, status } from "holon"
import payouts from "collection:payouts"
import ops from "mail:ops"
import { now } from "holon:clock"

flow("approval-mail", { port: 8186 })

// Where the flow is reached from outside: the base of the links it mails.
const site = "http://127.0.0.1:8186"

// Starts the payout's run and mails its approval link to whoever approves payouts.
route.post("/payouts", (req) => {
  const asked = { id: req.json.id, amount: req.json.amount, iban: req.json.iban, by: req.json.by }
  const started = payout(asked)
  ops.send({
    to: "approvals@acme.com",
    subject: `Approve payout ${asked.id}: ${asked.amount} to ${asked.iban}`,
    text: `${asked.by} asks to pay ${asked.amount} to ${asked.iban}.\n\nApprove it within a day: ${site}/approve/${started.waiting.approved}\n`,
    attachments: {
      "payout.csv": `id,amount,iban,by\n${asked.id},${asked.amount},${asked.iban},${asked.by}\n`,
    },
  })
  return status(202, { id: asked.id, waiting: "approval" })
})

// The payout's run: waits for its approval, then records it and tells whoever asked.
export const payout = action(async (asked) => {
  const approved = await event("approved", { expires: "1d" })
  payouts[asked.id] = { ...asked, approved: now, approved_via: approved.method }
  ops.send({
    to: asked.by,
    subject: `Payout ${asked.id} approved`,
    text: `Your payout of ${asked.amount} to ${asked.iban} was approved, and is recorded.\n`,
  })
})

// The page an approval link opens. Opening it approves nothing; its button does. Unrecorded: its
// path holds the approval's token, which anyone reading the log could approve with.
route.get(
  "/approve/:token",
  () =>
    respond(
      `<!doctype html><title>Approve payout</title><form method="post"><p>Approve this payout?</p><button>Approve</button></form>`,
      { headers: { "content-type": "text/html; charset=utf-8" } },
    ),
  { log: "unrecorded" },
)

// The button: approves the waiting payout.
route.post(
  "/approve/:token",
  () =>
    respond(`<!doctype html><title>Approved</title><p>Approved.</p>`, {
      headers: { "content-type": "text/html; charset=utf-8" },
    }),
  { resumes: "approved" },
)

// The payouts approved.
route.get("/payouts", () => payouts.list({ limit: 50 }))
