// Billing (samples/billing.holon) in TypeScript's syntax: the same unit.
import { action, flow, route } from "holon"
import charges from "collection:charges"

flow("billing", { port: 8158 })

// Records a charge with its fee, and answers its total.
export const charge = action((input) => {
  charges[input.id] = { amount: input.amount, fee: 1 }
  return { id: input.id, total: input.amount + 1 }
})

// Removes a charge.
export const refund = action((input) => {
  delete charges[input.id]
})

route.get("/charges/:id", (req) => charges[req.params.id])
