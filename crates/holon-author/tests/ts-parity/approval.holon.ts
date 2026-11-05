// Order approval (samples/approval.holon) in TypeScript's syntax: the same unit.
import { flow, route, action, event } from "holon"
import state from "state"
import orders from "channel:orders"

flow("approval", { port: 8105 })

// Starts the order's run, calling it without await, and answers its pay link: the token that
// resumes it where it waits.
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
