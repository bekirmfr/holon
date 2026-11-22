// A key written, then written again inside an if whose test is missing.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{}' http://127.0.0.1:9652/go
// JavaScript:  writes k = "two"; the if is not entered.
// Then look:   curl http://127.0.0.1:9652/count
import { flow, route } from "holon"
import state from "state"

flow("case-12-if-on-nothing-undoes-write", { port: 9652 })

route.post("/go", (req) => {
  state.k = "two"
  if (req.json.missing) {
    state.k = "one"
  }
  return "end"
})

route.get("/count", () => ({ k: state.k ?? "none" }))
