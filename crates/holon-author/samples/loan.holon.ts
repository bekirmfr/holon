// Loan: JavaScript's numbers.
//
// The monthly payment on a loan, from its amount, its yearly rate in percent and its term in
// months, computed as JavaScript computes it: parseFloat and parseInt read the query's texts, **
// compounds the rate, Math.round rounds to the cent, toFixed writes the cents, and Math.floor and
// % split the term into years and months. A part that isn't a number has no value, where
// JavaScript would give NaN, so the route answers 400.
//
// Try it: deploy it, then
//   curl "http://127.0.0.1:8173/payment?amount=250000&rate=6.5&months=360"
//     {"interest":"318861.20","payment":"1580.17","term":"30y 0m","total":"568861.20"}
//   curl "http://127.0.0.1:8173/payment?amount=lots&rate=6.5&months=360"    400
import { flow, route, status } from "holon"

flow("loan", { port: 8173 })

// The payment, the total paid, the interest in it and the term; 400 when a part isn't a number.
route.get("/payment", (req) => {
  const amount = parseFloat(req.query.amount)
  const monthly = parseFloat(req.query.rate) / 100 / 12
  const months = parseInt(req.query.months)
  const payment = monthly === 0
    ? amount / months
    : amount * monthly / (1 - 1 / (1 + monthly) ** months)
  const cents = Math.round(payment * 100)
  const total = cents * months / 100
  const loan = {
    payment: (cents / 100).toFixed(2),
    total: total.toFixed(2),
    interest: (total - amount).toFixed(2),
    term: `${Math.floor(months / 12)}y ${months % 12}m`,
  }
  return loan ?? status(400)
})
