// Notes API (samples/notes-api.holon) in TypeScript's syntax: the same unit.
import { flow, route, status } from "holon"
import notes from "collection:notes"

flow("notes-api", { port: 8102 })

// Lists the notes, 50 at a time, after the key the caller names.
route.get("/notes", (req) => notes.list({ after: req.query.after ?? "", limit: 50 }), { log: "lazy" })

// Saves the note sent; 400 when the body isn't JSON.
route.put("/notes/:id", (req) => {
  notes[req.params.id] = req.json
  return req.json ?? status(400)
})

route.get("/notes/:id", (req) => notes[req.params.id])

route.delete("/notes/:id", (req) => {
  delete notes[req.params.id]
  return status(204)
})
