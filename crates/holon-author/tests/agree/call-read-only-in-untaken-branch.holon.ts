// A start in a choice that is taken, its result read only in a branch that is not.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"c":true,"d":false}' http://127.0.0.1:9649/go
// JavaScript:  starts ping (c is true), then answers "no".
// Then look:   curl http://127.0.0.1:9649/count
import { action, flow, route } from "holon"
import state from "state"

flow("case-09-call-read-only-in-untaken-branch", { port: 9649 })

export const ping = action((x) => ++state.pinged)

route.post("/go", (req) => {
  const got = req.json.c ? ping(req.json) : 0
  return req.json.d ? got : "no"
})

route.get("/count", () => ({ pinged: state.pinged ?? 0 }))
