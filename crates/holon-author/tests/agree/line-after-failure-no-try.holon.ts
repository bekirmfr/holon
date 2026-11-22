// A write on the line after one that fails, with no try around them.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"text":"{bad"}' http://127.0.0.1:9651/go
// JavaScript:  throws at JSON.parse; the next line never runs, n stays 0.
// Then look:   curl http://127.0.0.1:9651/count
import { flow, route } from "holon"
import state from "state"

flow("case-11-line-after-failure-no-try", { port: 9651 })

route.post("/go", (req) => {
  const parsed = JSON.parse(req.json.text)
  ++state.n
  return "end"
})

route.get("/count", () => ({ n: state.n ?? 0 }))
