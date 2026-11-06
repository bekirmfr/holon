// Atlas notes: notes kept in a MongoDB database.
//
// Keeps each owner's notes as documents in a MongoDB collection an admin binds on the host: a
// cluster on MongoDB Atlas, or a replica set of your own. Each filter and update is a record written
// in the flow, and the request's values go in as data, so a request can never add an operator.
// Finds answer what the run found; inserts, updates and deletes land when the run finishes, in one
// transaction that first checks what it found is still so.
//
// Before it works, start the server with the database's address in its environment, either
//   HOLON_SECRET_NOTES_URL=mongodb+srv://user:password@cluster0.example.mongodb.net/notes (Atlas)
//   HOLON_SECRET_NOTES_URL=mongodb://127.0.0.1:27017/notes?replicaSet=rs0 (your own)
// and bind it (an admin):
//   holon database add notes env:HOLON_SECRET_NOTES_URL
//
// Try it: deploy it, then
//   curl -X POST -d '{"id":"n1","text":"buy milk","tags":["home"]}' http://127.0.0.1:8154/notes/ada
//   curl http://127.0.0.1:8154/notes/ada                    ada's open notes
//   curl http://127.0.0.1:8154/notes/ada/tagged/home        ada's notes tagged home
//   curl -X POST http://127.0.0.1:8154/notes/ada/n1/done    twice: the second finds no open note
import { flow, route, status } from "holon"
import notes from "database:notes"

flow("atlas-notes", { port: 8154 })

// Adds a note for an owner, open and not yet edited.
route.post("/notes/:owner", (req) => {
  notes.insert("notes", {
    _id: req.json.id,
    owner: req.params.owner,
    text: req.json.text,
    tags: req.json.tags ?? [],
    done: false,
    edits: 0,
  })
  return status(201, { added: req.json.id })
})

// An owner's open notes, in id order, at most 50.
route.get("/notes/:owner", (req) =>
  notes.find("notes", { owner: req.params.owner, done: false }, { sort: { _id: 1 }, limit: 50 }),
)

// An owner's notes that carry a tag, open or done, in id order.
route.get("/notes/:owner/tagged/:tag", (req) =>
  notes.find(
    "notes",
    { owner: req.params.owner, tags: { "$in": [req.params.tag] } },
    { sort: { _id: 1 } },
  ),
)

// Replaces a note's text and counts the edit; it lands only if it matches exactly one note.
route.put("/notes/:owner/:id", (req) => {
  notes.update(
    "notes",
    { _id: req.params.id, owner: req.params.owner },
    { "$set": { text: req.json.text }, "$inc": { edits: 1 } },
    { expect: 1 },
  )
  return { edited: req.params.id }
})

// Marks an open note done and answers its text; a note already done finds none, and nothing lands.
route.post("/notes/:owner/:id/done", (req) => {
  const note = notes.find_one("notes", { _id: req.params.id, owner: req.params.owner, done: false })
  notes.update("notes", { _id: note._id }, { "$set": { done: true } }, { expect: 1 })
  return { done: note._id, text: note.text }
})

// Deletes an owner's note; it lands only if it deletes exactly one.
route.delete("/notes/:owner/:id", (req) => {
  notes.delete("notes", { _id: req.params.id, owner: req.params.owner }, { expect: 1 })
  return status(204)
})
