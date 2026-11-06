// Balance trail: which run changed a value.
//
// Moves money in and out of accounts: a balance per account in a collection, and a count of
// moves in the flow's state. A cell's history answers which run changed a value: the runs that
// changed one balance (or the count, or any account), newest first, each with the value before
// and after it. A restore between two runs shows as a row of its own, so each value follows from
// the one before.
//
// Try it: deploy it, then
//   curl -X POST -d '{"amount":100}' http://127.0.0.1:8162/accounts/a/move
//   curl -X POST -d '{"amount":5}' http://127.0.0.1:8162/accounts/b/move
//   curl -X POST -d '{"amount":-30}' http://127.0.0.1:8162/accounts/a/move
//   curl http://127.0.0.1:8162/accounts/a                   balance 70
//   holon history balance-trail --cell balances --item a    the two runs that moved a, not b's
//   holon history balance-trail --cell moves                all three runs
// Put back the state the first run left (holon restore balance-trail --to-run SEQ --state) and
// move a by 1: its history shows the restore (70 to 100), then the new run (100 to 101).
// In the editor: view mode, pick a run, Flow state, then ⧗ beside balances or one of its items.
import { flow, route } from "holon"
import state from "state"
import balances from "collection:balances"

flow("balance-trail", { port: 8162 })

// Adds the amount to the account's balance, counts the move, and answers the new balance.
route.post("/accounts/:id/move", (req) => {
  const balance = (balances[req.params.id] ?? 0) + req.json.amount
  balances[req.params.id] = balance
  state.moves = (state.moves ?? 0) + 1
  return { account: req.params.id, balance }
})

// An account's balance, 0 for one that never moved.
route.get("/accounts/:id", (req) => ({
  account: req.params.id,
  balance: balances[req.params.id] ?? 0,
}))
