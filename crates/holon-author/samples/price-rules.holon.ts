// Price rules: a new version tried on the runs it would have answered.
//
// Prices an order: a member pays 10% less, and an order without a total is refused. Once it has
// answered a few orders, a new version (a bigger discount, a minimum order) can be replayed on
// them before it goes live: the deploy's dialog and the version's review say how many runs it
// answers the same, which it answers differently, and how. The replay is offline: nothing is
// sent, and nothing it would write is written.
//
// Try it: deploy it, then
//   curl -X POST -H "Content-Type: application/json" -d '{"total":80,"member":true}' \
//     http://127.0.0.1:8185/price                                     {"total":80,"price":72}
//   curl -X POST -H "Content-Type: application/json" -d '{"total":80}' \
//     http://127.0.0.1:8185/price                                     {"total":80,"price":80}
//   curl -X POST -H "Content-Type: application/json" -d '{}' http://127.0.0.1:8185/price   422
// Then change 0.9 to 0.85 and Deploy: before it goes live, "3 replayed · 2 same · 1 differs",
// the member's order answering 68 where it answered 72.
import { flow, route, status } from "holon"

flow("price-rules", { port: 8185 })

// A member's order costs 10% less; one without a total is refused.
route.post("/price", (req) => {
  const total = req.json.total ?? 0
  if (!(total > 0)) {
    return status(422, "an order has a total")
  }
  return { total, price: (req.json.member ?? false) ? total * 0.9 : total }
})
