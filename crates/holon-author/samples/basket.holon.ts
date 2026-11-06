// Basket: a collection read inside a function.
//
// Prices a basket from a price list kept in a collection. Each line's function reads its item's
// price, so the read happens once per item, each answered on its own, recorded and replayed. An
// item with no price answers 404 instead of a made-up total.
//
// Try it: deploy it, then
//   curl -X PUT -d '{"price":4}' http://127.0.0.1:8113/prices/tea
//   curl -X PUT -d '{"price":3}' http://127.0.0.1:8113/prices/cake
//   curl -X POST -d '{"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}]}' \
//     http://127.0.0.1:8113/basket
// answers each line's total and a total of 11. An item with no price answers 404.
import { flow, route } from "holon"
import prices from "collection:prices"

flow("basket", { port: 8113 })

// Sets one item's price.
route.put("/prices/:item", (req) => {
  prices[req.params.item] = req.json.price
  return { item: req.params.item, price: req.json.price }
})

// Prices each line from the list, then adds the lines up.
route.post("/basket", (req) => {
  const lines = req.json.items.map((item) => ({
    name: item.name,
    total: prices[item.name] * item.qty,
  }))
  return { lines, total: lines.reduce((acc, line) => acc + line.total, 0) }
})
