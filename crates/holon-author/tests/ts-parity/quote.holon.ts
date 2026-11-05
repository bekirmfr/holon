// Quote (samples/quote.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"

flow("quote", { port: 8112 })

// Prices each item with the tax (none if it isn't sent), and sums the lines.
route.post("/quote", (req) => {
  const tax = req.json.tax ?? 0
  const lines = req.json.items.map((item) => ({ name: item.name, total: item.price * item.qty * (1 + tax) }))
  const total = lines.reduce((acc, line) => acc + line.total, 0)
  return { lines, total, tax }
})
