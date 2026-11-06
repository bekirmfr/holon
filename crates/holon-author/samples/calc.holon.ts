// Calculator: a switch.
//
// Answers a + b, a - b, a * b or a / b, by the op it is sent. A switch picks one case, and the
// others don't run. When the chosen case has no value (a division by zero), its answer has none
// either, so the route answers 400 rather than some other case's value. An op with no case
// answers 400 too.
//
// Try it: deploy it, then
//   curl -d '{"op":"add","a":1,"b":0}' http://127.0.0.1:8116/calc    1
//   curl -d '{"op":"div","a":1,"b":0}' http://127.0.0.1:8116/calc    400
import { flow, route, status } from "holon"

flow("calc", { port: 8116 })

// Answers the value of the op's case, or 400 when it has none.
route.post("/calc", (req) => {
  const a = req.json.a
  const b = req.json.b
  switch (req.json.op) {
    case "add":
      return a + b ?? status(400)
    case "sub":
      return a - b ?? status(400)
    case "mul":
      return a * b ?? status(400)
    case "div":
      return a / b ?? status(400)
  }
  return status(400)
})
