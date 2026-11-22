// A function writes inside a try that then fails.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"a":"x","text":"{bad"}' http://127.0.0.1:9642/go
// JavaScript:  throws at JSON.parse, lands in catch; Holon's promise: a failed try keeps nothing, so n stays 0.
// Then look:   curl http://127.0.0.1:9642/count
import { flow, route } from "holon"
import state from "state"

flow("case-02-failed-try-keeps-function-write", { port: 9642 })

function bump(x) {
  ++state.n
  return x + "!"
}

route.post("/go", (req) => {
  try {
    const n = bump(req.json.a)
    const parsed = JSON.parse(req.json.text)
  } catch {
    return "bad"
  }
  return "end"
})

route.get("/count", () => ({ n: state.n ?? 0 }))
