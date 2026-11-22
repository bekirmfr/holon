// A function that starts something, called under an if.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"go":false,"a":"x"}' http://127.0.0.1:9641/go
// JavaScript:  starts nothing, because the if is false.
// Then look:   curl http://127.0.0.1:9641/count
import { action, flow, route } from "holon"
import state from "state"

flow("case-01-function-under-if", { port: 9641 })

export const ping = action((x) => ++state.pinged)

function ask(x) {
  if (x) {
    ping(x)
  }
  return 1
}

route.post("/go", (req) => {
  if (req.json.go) {
    const asked = ask(req.json.a)
  }
  return "end"
})

route.get("/count", () => ({ pinged: state.pinged ?? 0 }))
