// An action answers early, above its wait; a later step uses what the wait gave.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"stop":true}' http://127.0.0.1:9647/go
// JavaScript:  answers "stopped" and never waits.
import { action, event, flow, route } from "holon"
import updates from "channel:updates"

flow("case-07-early-answer-above-wait", { port: 9647 })

export const pay = action(async (input) => {
  if (input.stop) return "stopped"
  const ok = await event("approved", { expires: "1h" })
  updates.send({ status: "paid", by: ok.by })
  return "done"
})

route.post("/go", (req) => {
  const started = pay(req.json)
  return started
})
