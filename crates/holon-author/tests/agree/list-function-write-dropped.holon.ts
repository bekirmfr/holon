// A function that writes, called for each item of a list.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"a":"x"}' http://127.0.0.1:9643/go
// JavaScript:  writes n once, one item in the list.
// Then look:   curl http://127.0.0.1:9643/count
import { flow, route } from "holon"
import state from "state"

flow("case-03-list-function-write-dropped", { port: 9643 })

function keep(x) {
  ++state.n
  return x + "!"
}

route.post("/go", (req) => {
  const got = [req.json.a].map((x) => keep(x))
  return "end"
})

route.get("/count", () => ({ n: state.n ?? 0 }))
