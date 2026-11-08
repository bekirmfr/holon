// Due dates: business days over a calendar given as data, at a fixed offset.
//
// When a payout is executed and credited: received before the 16:00 cut-off at the bank's offset,
// it is executed that business day, else the next, and credited one business day later. And when
// an invoice is due: some months after it was issued, the day of the month kept or the month's
// last, moved to the next business day.
//
// The closing days are TARGET2's in 2026, a list the flow holds: a bank's calendar is data, not
// built in. The offset is fixed: Frankfurt is +01:00 in winter and +02:00 in summer, so a request
// may say which; the host has no time zones.
//
// Try it: deploy it, then, after the cut-off on the Thursday before Easter,
//   curl -d '{"received":"2026-04-02T16:30:00+02:00","offset":"+02:00"}' http://127.0.0.1:8181/schedule
//     {"credited":"2026-04-08","executed":"2026-04-07","received":"2026-04-02T16:30:00.000+02:00"}
// (Good Friday and Easter Monday are closed), and for an invoice of January 31,
//   curl -d '{"issued":"2026-01-31","months":1}' http://127.0.0.1:8181/due
//     {"business_days":21,"due":"2026-03-02"}
// (February 28 is a Saturday).
import {
  business_day_add,
  business_days_between,
  date_add,
  date_part,
  date_text,
  flow,
  route,
} from "holon"
import { now } from "holon:clock"

flow("due-dates", { port: 8181 })

// TARGET2's closing days in 2026.
function closed() {
  return ["2026-01-01", "2026-04-03", "2026-04-06", "2026-05-01", "2026-12-25", "2026-12-26"]
}

// When a payout received now, or when the request says, is executed and credited.
route.post("/schedule", (req) => {
  const offset = req.json.offset ?? "+01:00"
  const received = req.json.received ?? now * 1000
  const late = date_part(received, offset, { part: "hours" }) >= 16
  const today = date_text(received, offset, { format: "date" })
  const executed = business_day_add(today, late ? 1 : 0, closed())
  return {
    received: date_text(received, offset),
    executed,
    credited: business_day_add(executed, 1, closed()),
  }
})

// When an invoice issued on a date is due, some months later, and how many business days that is.
route.post("/due", (req) => {
  const later = date_add(req.json.issued, req.json.months, { unit: "months" })
  const due = business_day_add(date_text(later, { format: "date" }), 0, closed())
  return { due, business_days: business_days_between(req.json.issued, due, closed()) }
})
