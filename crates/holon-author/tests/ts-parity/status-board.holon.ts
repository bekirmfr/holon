// Status board (samples/status-board.holon) in TypeScript's syntax: the same unit, each handler's
// log level its `log` option.
import { action, flow, route } from "holon"
import notes from "collection:notes"
import seen from "collection:seen"

flow("status-board", { port: 8170 })

route.get("/health", () => "ok", { log: "unrecorded" })

route.get("/notes", () => notes.list({ limit: 50 }), { log: "lazy" })

route.post("/notes", (req) => {
  notes[req.json.id] = req.json.text
  return "saved"
})

route.post("/notes/:id/seen", (req) => {
  seen[req.params.id] = req.json.by
  return "ok"
}, { log: "lazy" })

export const report = action(() => notes.list({ limit: 50 }), { log: "unrecorded" })
