// Restock: a request and a write for each item of a list.
//
// Takes a restock order of several lines and, for each line in turn, asks the supplier for it and
// keeps the line in the stock collection: one request and one write per item, made in the items'
// order, as JavaScript's map makes them. Lines after one whose function fails are not asked. A list
// longer than the run may send requests for (50 by default) fails before any is sent.
//
// Try it: deploy it (holon deploy restock.holon.ts), then
//   curl -X POST -d '{"lines":[{"sku":"a1","qty":3},{"sku":"b2","qty":5}]}' http://127.0.0.1:8204/restock
// answers the skus asked for, and the run log shows two requests and two writes, a1's first.
import { flow, route } from "holon"
import stock from "collection:stock"
import httpbin from "host:httpbin.org"

flow("restock", { port: 8204 })

// Asks the supplier for one line, and keeps it with what the supplier answered.
async function reorder(line) {
  const asked = await httpbin.post("/anything/restock", {
    headers: { "content-type": "application/json" },
    body: JSON.stringify(line),
  })
  stock.put(line.sku, { qty: line.qty, status: asked.status })
  return line.sku
}

route.post("/restock", (req) => {
  const asked = req.json.lines.map((line) => reorder(line))
  return { asked }
})
