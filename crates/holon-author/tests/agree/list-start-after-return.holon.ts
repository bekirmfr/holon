// A list's function starts something, below a return that was taken.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"stop":true,"a":"x"}' http://127.0.0.1:9644/go
// JavaScript:  answers "early" and starts nothing.
// Then look:   curl http://127.0.0.1:9644/count
import { action, flow, route } from "holon"
import state from "state"

flow("case-04-list-start-after-return", { port: 9644 })

export const ping = action((x) => ++state.pinged)

route.post("/go", (req) => {
  if (req.json.stop) return "early"
  const got = [req.json.a].map((x) => ping(x))
  return "end"
})

route.get("/count", () => ({ pinged: state.pinged ?? 0 }))
