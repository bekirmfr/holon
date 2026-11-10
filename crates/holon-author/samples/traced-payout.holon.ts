// Traced payout: a flow that says what it is doing, with the console's own methods.
//
// Asks for a payout, { "id": "p-1", "amount": 250 }, and answers it, refusing one over 1000. As it
// goes it writes console.log, console.debug and console.warn lines. The host keeps them beside the
// deployment's runs: none is an effect a run performed, `holon verify` checks a run without them,
// and a flow can't read them back. A deployment keeps a few MB of them, the oldest
// dropped first, and a line says when some were. People read them; an agent only where the
// deployment's grant lets it.
//
// Try it: deploy it, ask for two payouts, and read what they wrote:
//   curl -X POST -d '{"id":"p-1","amount":250}' http://127.0.0.1:8147/payout
//   curl -X POST -d '{"id":"p-2","amount":5000}' http://127.0.0.1:8147/payout   refused: over the limit
//   holon logs traced-payout                  each line, with its run and its level
//   holon logs traced-payout --level warn     only the warning
//   holon logs traced-payout --follow         keeps reading as new lines come
import { flow, route, status } from "holon"

flow("traced-payout", { port: 8147 })

route.post("/payout", (req) => {
  console.log("payout asked", req.json.id, req.json.amount)
  if (req.json.amount > 1000) {
    console.warn("over the usual limit", req.json.id)
    return status(409, { id: req.json.id, refused: "over the limit" })
  }
  console.debug("paying", req.json.id)
  return { id: req.json.id, paid: true }
})
