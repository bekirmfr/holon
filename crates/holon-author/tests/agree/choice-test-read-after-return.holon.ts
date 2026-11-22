// A choice whose test reads a collection, below a return that was taken.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"stop":true,"k":"a"}' http://127.0.0.1:9648/go
// JavaScript:  answers "stopped" and reads nothing.
// Then look:   curl http://127.0.0.1:9648/count
import { flow, route } from "holon"
import state from "state"
import notes from "collection:notes"

flow("case-08-choice-test-read-after-return", { port: 9648 })

function rec(x) {
  ++state.f
  return x + "!"
}

route.post("/go", (req) => {
  if (req.json.stop) return "stopped"
  const got = notes[req.json.k] === "v" ? rec(req.json.k) : 0
  return "end"
})

route.get("/count", () => ({ f: state.f ?? 0 }))
