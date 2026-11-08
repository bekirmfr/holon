// Intake: try, catch, throw and finally.
//
// Takes orders as JSON. total() throws when an order has no lines, or its total isn't more than
// 0, and the route's try { ... } catch { ... } answers 400 for that, or for a body that isn't JSON,
// or for a line without a price (catch catches a value that has none, as much as a throw). The
// order is stored only when the whole try block succeeded. finally counts every attempt, kept or
// not. A throw's message goes to the run's log. /check reads total() without a try: a throw
// nothing catches is nothing, which its ?? answers.
//
// Try it: deploy it, then
//   curl -X POST -d '{"id":"o1","lines":[{"price":2},{"price":3}]}' \
//     http://127.0.0.1:8178/orders                            201 {"id":"o1","total":5}
//   curl -X POST -d '{"id":"o2","lines":[]}' http://127.0.0.1:8178/orders
//                                                             400, "an order has lines" in the log
//   curl -X POST -d 'not json' http://127.0.0.1:8178/orders   400, and nothing kept
//   curl http://127.0.0.1:8178/orders/o1                      the order
//   curl http://127.0.0.1:8178/stats                          {"tried":3}
//   curl -X POST -H "Content-Type: application/json" -d '{"lines":[]}' \
//     http://127.0.0.1:8178/check                             422
import { flow, length, route, status } from "holon"
import orders from "collection:orders"
import state from "state"

flow("intake", { port: 8178 })

// An order's total, or a throw saying what is wrong with it.
function total(order) {
  if (length(order.lines) === 0) throw new Error("an order has lines")
  const sum = order.lines.reduce((s, line) => s + line.price, 0)
  if (sum <= 0) throw new Error("an order's total is more than 0")
  return sum
}

// Keeps an order only when every step of the try block succeeded; counts every attempt.
route.post("/orders", (req) => {
  let order
  try {
    const body = JSON.parse(req.body)
    order = { id: body.id, total: total(body) }
    orders[body.id] = order
  } catch {
    return status(400, { error: "not an order: the run's log says why" })
  } finally {
    state.tried = (state.tried ?? 0) + 1
  }
  return status(201, order)
})

// Checks an order without keeping it.
route.post("/check", (req) => status(200, { total: total(req.json) }) ?? status(422))

// One order, as it was kept.
route.get("/orders/:id", (req) => orders[req.params.id])

// How many orders were sent, kept or not.
route.get("/stats", () => ({ tried: state.tried ?? 0 }))
