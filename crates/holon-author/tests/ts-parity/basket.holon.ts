// Basket (samples/basket.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"
import prices from "collection:prices"

flow("basket", { port: 8113 })

// Sets an item's price.
route.put("/prices/:item", (req) => {
  prices[req.params.item] = req.json.price
  return { item: req.params.item, price: req.json.price }
})

// Prices each line from the collection, and sums them.
route.post("/basket", (req) => {
  const lines = req.json.items.map((item) => ({
    name: item.name,
    total: prices[item.name] * item.qty,
  }))
  return { lines, total: lines.reduce((acc, line) => acc + line.total, 0) }
})
