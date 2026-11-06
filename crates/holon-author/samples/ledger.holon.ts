// Ledger: every run checkable.
//
// Keeps a balance per account: each post adds its amount and answers the new balance with when it
// was posted. Export its runs, and holon verify runs every posting again, offline, from what its
// log entry recorded (the balance it read, the clock). It checks each produced what the log says,
// and that each began from the balances the one before left.
//
// Try it: deploy it (holon deploy ledger.holon.ts), then a few times
//   curl -X POST -d '{"amount":5}' http://127.0.0.1:8131/accounts/ada
// Save GET /deployments/<slug>/export (signed in) as runs.json, then
//   holon verify runs.json
import { flow, route, status } from "holon"
import balances from "collection:balances"
import { now } from "holon:clock"

flow("ledger", { port: 8131 })

// Adds the posted amount to the account's balance and answers it (400 without an amount).
route.post("/accounts/:id", (req) => {
  const account = req.params.id
  const balance = (balances[account] ?? 0) + req.json.amount
  balances[account] = balance
  return { balance, posted_at: now } ?? status(400)
})

// An account's balance.
route.get("/accounts/:id", (req) => balances[req.params.id])
