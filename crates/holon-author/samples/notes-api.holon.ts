// Notes API: notes kept in a collection, by id.
//
// Create, read, list and delete notes kept in a collection, keyed by id. The list comes a page of
// 50 at a time; ?after=<id> gives the page after that id. A note that isn't there answers 404.
//
// Try it: deploy it, then
//   curl -X PUT -H "Content-Type: application/json" -d '{"text":"buy milk"}' \
//     http://127.0.0.1:8102/notes/1
//   curl http://127.0.0.1:8102/notes              (?after=1 for the next page)
//   curl http://127.0.0.1:8102/notes/1
//   curl -X DELETE http://127.0.0.1:8102/notes/1
// curl http://127.0.0.1:8102/notes/9 fails (404): open the run in view mode to see where, and why.
import { flow, route, status } from "holon"
import notes from "collection:notes"

flow("notes-api", { port: 8102 })

// A page of notes, in key order, after the id given (from the start without one). It only reads,
// so it is lazy: recorded, on disk within a second of its answer.
route.get(
  "/notes",
  (req) => notes.list({ after: req.query.after ?? "", limit: 50 }),
  { log: "lazy" },
)

// Keeps the posted JSON as the note with this id, replacing any before it.
route.put("/notes/:id", (req) => {
  notes[req.params.id] = req.json
  return req.json ?? status(400)
})

// One note, or 404 when there is none.
route.get("/notes/:id", (req) => notes[req.params.id])

// Deletes the note and answers 204, with no body.
route.delete("/notes/:id", (req) => {
  delete notes[req.params.id]
  return status(204)
})
