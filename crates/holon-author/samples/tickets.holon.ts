// Tickets: counting to n.
//
// Hands out count numbered tickets for an event: range counts from 0 to count-1, and a function
// names each one, using the event from around it. range stops at 10000, so a map over it always
// ends; a count it can't count to answers 400.
//
// Try it: deploy it (holon deploy tickets.holon.ts), then
//   curl -X POST -d '{"event":"gig","count":3}' http://127.0.0.1:8114/tickets
// answers gig #1, gig #2 and gig #3; a count of 2.5, -1 or 20000 answers 400.
import { flow, range, route, status } from "holon"

flow("tickets", { port: 8114 })

// Names count tickets for the event, numbered from 1.
route.post("/tickets", (req) => {
  const event = req.json.event
  return range(req.json.count).map((k) => `${event} #${k + 1}`) ?? status(400)
})
