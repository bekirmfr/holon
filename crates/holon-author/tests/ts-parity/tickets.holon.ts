// Tickets (samples/tickets.holon) in TypeScript's syntax: the same unit.
import { flow, range, route, status } from "holon"

flow("tickets", { port: 8114 })

// Names count tickets for the event, numbered from 1.
route.post("/tickets", ({ json }) => {
  const event = json.event
  return range(json.count).map((k) => `${event} #${k + 1}`) ?? status(400)
})
