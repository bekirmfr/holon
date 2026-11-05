// Calculator (samples/calc.holon) in TypeScript's syntax: the same unit. Each case answers 400
// when its value is absent (a division by zero), and an op with no case answers 400 too: the
// switch is the sample's match, with missing: 400.
import { flow, route, status } from "holon"

flow("calc", { port: 8116 })

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
