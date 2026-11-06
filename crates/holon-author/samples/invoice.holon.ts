// Invoice: records and dates.
//
// Post an order and get its invoice, made with JavaScript's record and date functions: the
// order's defaults under what was posted, with a spread; its lines priced and grouped by
// category with Object.groupBy, each group's subtotal with Object.fromEntries; whether it is a
// rush order with in; the date it was issued, the date it is due (setUTCDate, a day past the
// month's end carried into the next) and the last day of the month (Date.UTC with day 0), as ISO
// and UTC text. POST /invoice/lines answers the order as JSON text with only the listed keys, at
// every depth, indented: JSON.stringify with a key list and an indent.
//
// Try it: deploy it, then
//   curl -d '{"customer":"ada","date":"2026-11-07","lines":[{"sku":"b1","category":"books","price":12,"qty":2},{"sku":"p1","category":"pens","price":3,"qty":5},{"sku":"b2","category":"books","price":30,"qty":1}]}' http://127.0.0.1:8175/invoice
//     {"categories":["books","pens"],"currency":"EUR","customer":"ada","due":"2026-12-07T00:00:00.000Z","due_text":"Mon, 07 Dec 2026 00:00:00 GMT","issued":"2026-11-07T00:00:00.000Z","month_end":"2026-11-30T00:00:00.000Z","rush":false,"subtotals":{"books":54,"pens":15},"total":69}
//   the same body to http://127.0.0.1:8175/invoice/lines answers the customer and each line's sku
//   and qty, indented by two spaces; a body without lines to /invoice    400
import { flow, route, status } from "holon"

flow("invoice", { port: 8175 })

// The invoice: defaults under the order, lines grouped by category, the dates it is issued and due.
route.post("/invoice", (req) => {
  const order = { currency: "EUR", terms_days: 30, ...req.json }
  const lines = order.lines.map((l) => ({ ...l, total: l.price * l.qty }))
  const groups = Object.groupBy(lines, (l) => l.category)
  const issued = new Date(order.date)
  const due = new Date(order.date)
  due.setUTCDate(due.getUTCDate() + order.terms_days)
  const last = new Date(Date.UTC(issued.getUTCFullYear(), issued.getUTCMonth() + 1, 0))
  const invoice = {
    customer: order.customer,
    currency: order.currency,
    categories: Object.keys(groups),
    subtotals: Object.fromEntries(
      Object.keys(groups).map((k) => [k, groups[k].reduce((sum, l) => sum + l.total, 0)]),
    ),
    total: lines.reduce((sum, l) => sum + l.total, 0),
    rush: "rush" in order,
    issued: issued.toISOString(),
    due: due.toISOString(),
    due_text: due.toUTCString(),
    month_end: last.toISOString(),
  }
  return invoice ?? status(400)
})

// The order as JSON text: only the customer and each line's sku and qty, two spaces in.
route.post("/invoice/lines", (req) =>
  JSON.stringify(req.json, ["customer", "lines", "sku", "qty"], 2),
)
