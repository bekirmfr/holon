// Shop: a flow calling another's actions.
//
// Runs another deployed flow's actions. It imports billing's actions from
// "flow:billing", and charge(x) runs billing's action charge with x, answering {answer, run}, or
// {error, message} when billing can't be reached. Deploying it asks to run billing's actions
// charge and refund, which a person approves like any permission. An order is stored only when
// the charge answered, and the route answers 502 when billing can't be reached.
//
// Try it: deploy the sample billing first, then this one, then
//   curl -X POST -H "Content-Type: application/json" -d '{"id":"o1","amount":5}' \
//     http://127.0.0.1:8159/orders                    {"id":"o1","total":6}
//   curl http://127.0.0.1:8159/orders/o1              the order
//   curl -X DELETE http://127.0.0.1:8159/orders/o1    runs billing's refund
// billing's log shows the charge, run by shop. Stop billing and a new order answers 502.
import { flow, route, status } from "holon"
import orders from "collection:orders"
import { charge, refund } from "flow:billing"

flow("shop", { port: 8159 })

// Charges through billing, keeps the order with its total, and answers what billing answered.
route.post("/orders", async (req) => {
  const paid = await charge({ id: req.json.id, amount: req.json.amount })
  orders[req.json.id] = { amount: req.json.amount, total: paid.answer.total }
  return status(201, paid.answer) ?? status(502)
})

// Refunds the order through billing and removes it.
route.delete("/orders/:id", async (req) => {
  await refund({ id: req.params.id })
  delete orders[req.params.id]
  return status(204)
})

// One order, as it was stored.
route.get("/orders/:id", (req) => orders[req.params.id])
