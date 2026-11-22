// A write in a finally, below a return that was taken.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"stop":true,"text":"{}"}' http://127.0.0.1:9646/go
// JavaScript:  answers "stopped" and writes nothing: the try was never reached.
// Then look:   curl http://127.0.0.1:9646/count
import { flow, route } from "holon"
import state from "state"

flow("case-06-finally-after-return", { port: 9646 })

route.post("/go", (req) => {
  if (req.json.stop) return "stopped"
  try {
    const parsed = JSON.parse(req.json.text)
  } catch {
    return "bad"
  } finally {
    ++state.fin
  }
  return "end"
})

route.get("/count", () => ({ fin: state.fin ?? 0 }))
