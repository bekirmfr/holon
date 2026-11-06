// Order approval: a durable run.
//
// An order waits for its payment. The route that takes the order starts the order's run, an
// action, and answers with its pay link at once: a route answers and is done, and what waits is the
// run it starts. That run parks at the await and finishes when the link is opened, hours or days
// later (the link lasts a day). Its progress goes out on the channel orders as it happens:
// "waiting for payment" as the run parks, "paid" when it finishes, and never the first one twice.
//
// Try it: deploy it, then
//   curl -X POST -H "Content-Type: application/json" -d '{"item":"book"}' \
//     http://127.0.0.1:8105/orders
// answers /paid/<token>. Open http://127.0.0.1:8105/paid/<token> and the order is paid: it is
// written to the state as last_order. In another terminal,
//   curl -N http://127.0.0.1:8105/orders/live
// shows each order's progress: "waiting for payment", then "paid".
import { action, event, flow, route } from "holon"
import state from "state"
import orders from "channel:orders"

flow("approval", { port: 8105 })

// Starts the order's run and answers its pay link: the token that resumes it where it waits.
route.post("/orders", (req) => {
  const started = payment(req.json)
  return `/paid/${started.waiting.paid}`
})

// The order's run: says it is waiting, parks until the link is opened, then records the payment.
export const payment = action(async (order) => {
  orders.send({ item: order.item, status: "waiting for payment" })
  const paid = await event("paid", { expires: "1d" })
  state.last_order = { ...order, paid_via: paid.method }
  orders.send({ item: order.item, status: "paid", via: paid.method })
})

// The pay link: opening it wakes the waiting order's run.
route.get("/paid/:token", () => "thanks, the order is paid", { resumes: "paid" })

// Each order's progress as server-sent events, starting with the ones the channel keeps.
route.get("/orders/live", () => orders.subscribe({ from: "kept" }))
