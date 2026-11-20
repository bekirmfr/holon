// Refund review: an action that answers before it waits.
//
// A refund waits for a reviewer only when it needs one. Its run answers at once when the refund
// names no order or is small, and parks for a review otherwise. The answers that come before the wait
// are a row of `if (test) return value` right above it: where one is taken, the run answers and
// asks for no review at all. The canvas, the step view and both editors show the action this way.
//
// Try it: deploy it, then
//   curl -X POST -H "Content-Type: application/json" -d '{"order":"o-1","amount":5}' \
//     http://127.0.0.1:8137/refunds
// answers {"by":"rule","refunded":"o-1"} at once: nothing waits. With "amount":500 it answers
// /review/<token>; open http://127.0.0.1:8137/review/<token> and the refund goes ahead, written to
// the state as last_refund.
import { action, event, flow, route } from "holon"
import state from "state"

flow("refund-review", { port: 8137 })

// Starts the refund's run: what it answered, when it answered before waiting, else its review link.
// A body that isn't JSON is a refund naming nothing (`?? {}`): without it, nothing would start.
route.post("/refunds", (req) => {
  const started = refund(req.json ?? {})
  return started.answer ?? `/review/${started.waiting.reviewed}`
})

// The refund's run: one naming no order, or a small one, is answered before the wait; the rest
// park until a reviewer opens the link, and the refund is written then. Each test has a value
// whatever was sent (`?? ""`, `?? 0`): a test with none would end the run there, answering
// nothing and waiting for nothing.
export const refund = action(async (r) => {
  if ((r.order ?? "") === "") return { refused: "it names no order" }
  if ((r.amount ?? 0) <= 20) return { by: "rule", refunded: r.order }
  const reviewed = await event("reviewed", { expires: "2d" })
  state.last_refund = { ...r, reviewed_via: reviewed.method }
  return { by: "review", refunded: r.order }
})

// The review link: opening it lets the waiting refund go on.
route.get("/review/:token", () => "thanks, the refund goes ahead", { resumes: "reviewed" })
