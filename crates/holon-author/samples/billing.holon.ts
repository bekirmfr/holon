// Billing: actions another flow runs.
//
// The flow's work is two actions, run by name: charge records a charge in a collection and answers
// its total (the amount plus a fee of 1), refund deletes it. A person runs them from the editor;
// the sample shop runs them from its routes, a call from one flow to another's action. The port
// answers only the route, which reads a charge back.
//
// Try it: deploy it first, then the sample shop. Run charge from the editor's server panel with
//   {"id": "a1", "amount": 5}
// then
//   curl http://127.0.0.1:8158/charges/a1               the charge, with its fee
// In the deployed shop, an order's run shows up here as an action run by the shop.
import { action, flow, route } from "holon"
import charges from "collection:charges"

flow("billing", { port: 8158 })

// Records a charge and answers its total.
export const charge = action((input) => {
  charges[input.id] = { amount: input.amount, fee: 1 }
  return { id: input.id, total: input.amount + 1 }
})

// Deletes a charge, and answers nothing.
export const refund = action((input) => {
  delete charges[input.id]
})

// Reads a charge back.
route.get("/charges/:id", (req) => charges[req.params.id])
