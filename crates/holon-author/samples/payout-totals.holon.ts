// Payout totals: exact decimals, over a CSV.
//
// Runs once for each CSV of payouts (id, currency, amount) dropped into the folder payouts, and
// keeps each currency's count and total under the file's name. The amounts are added as decimals
// written as text, so they are exact: 0.10 and 0.20 make 0.30, where numbers would make
// 0.30000000000000004. Each total is rounded to cents half to even, as a bank rounds. A route
// reads a file's totals back.
//
// Before it works, bind the folder (an admin):
//   holon folder add payouts ~/holon-payouts --read-only
//
// Try it: deploy it, save this as ~/holon-payouts/may.csv
//   id,currency,amount
//   1,EUR,0.10
//   2,EUR,0.20
//   3,USD,1250.005
// then curl http://127.0.0.1:8180/totals/may.csv
//     {"EUR":{"count":2,"total":"0.30"},"USD":{"count":1,"total":"1250.00"}}
import { csv_parse, decimal_round, decimal_sum, flow, on, route } from "holon"
import totals from "collection:totals"
import payouts from "folder:payouts"

flow("payout-totals", { port: 8180 })

// One currency's payouts: how many, and their sum to the cent.
function summary(rows) {
  const amounts = rows.map((p) => p.amount)
  return { count: amounts.length, total: decimal_round(decimal_sum(amounts), 2) }
}

// A file's payouts, by currency.
on.file.created(payouts, (f) => {
  const groups = Object.groupBy(csv_parse(payouts[f.name]), (p) => p.currency)
  totals[f.name] = Object.fromEntries(Object.entries(groups).map((e) => [e[0], summary(e[1])]))
})

route.get("/totals/:name", (req) => totals[req.params.name])
