// A function whose start sits in a choice (x ? start : 0), called under an if.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"go":false,"a":"x"}' http://127.0.0.1:9645/go
// JavaScript:  starts nothing, because the if is false.
// Then look:   curl http://127.0.0.1:9645/count
import { action, flow, route } from "holon"
import state from "state"

flow("case-05-choice-inside-function", { port: 9645 })

export const ping = action((x) => ++state.pinged)

function maybe(x) {
  return x ? ping(x) : 0
}

route.post("/go", (req) => {
  if (req.json.go) {
    const made = maybe(req.json.a)
  }
  return "end"
})

route.get("/count", () => ({ pinged: state.pinged ?? 0 }))
