// A send whose content reads a collection, below a return that was taken.
//
// Send:        curl -X POST -H "Content-Type: application/json" -d '{"stop":true}' http://127.0.0.1:9650/go
// JavaScript:  answers "stopped", sends nothing and reads nothing.
import { flow, route } from "holon"
import notes from "collection:notes"
import updates from "channel:updates"

flow("case-10-read-after-return", { port: 9650 })

route.post("/go", (req) => {
  if (req.json.stop) return "stopped"
  updates.send({ text: notes["k"] })
  return "end"
})
