// Status board: how each handler's runs are recorded.
//
// Every run is recorded, and by default its record is on disk before the run answers: the handler
// is strict. Its author may say otherwise in its log option, on any handler:
//   lazy        recorded all the same, on disk within a second of the answer, so it answers
//               without waiting on the disk: for what must be fast, where losing the last second
//               of records to a crash is fine;
//   unrecorded  not in the log at all, failed or not: its runs are counted, with the last
//               failure's status and error. For what isn't worth a record: a health check, a
//               read, what is called so often it would only fill the log. A person approves it
//               at deploy, "keep runs of GET /health out of its log, failed or not".
// A handler that writes may be either: what it did is then as recorded as it says.
//
// Try it: deploy it (holon deploy status-board.holon.ts, approving what it asks), then
//   curl http://127.0.0.1:8170/health                                       "ok", counted
//   curl -X POST -d '{"id":"a","text":"hi"}' http://127.0.0.1:8170/notes    "saved", strict
//   curl -X POST -d '{"by":"sam"}' http://127.0.0.1:8170/notes/a/seen       "ok", lazy
//   curl http://127.0.0.1:8170/notes                                        the notes, lazy
// holon history status-board lists the notes runs, and the health checks as one counted line.
import { action, flow, route } from "holon"
import notes from "collection:notes"
import seen from "collection:seen"

flow("status-board", { port: 8170 })

// Answers the load balancer, every few seconds: not recorded, counted.
route.get("/health", () => "ok", { log: "unrecorded" })

// Lists the notes. Recorded, on disk within a second.
route.get("/notes", () => notes.list({ limit: 50 }), { log: "lazy" })

// Adds a note. Strict, the default: its record is what the change is undone and explained by.
route.post("/notes", (req) => {
  notes[req.json.id] = req.json.text
  return "saved"
})

// Marks a note seen, on every view: it writes, and is lazy all the same, as a lost second of
// these is fine.
route.post(
  "/notes/:id/seen",
  (req) => {
    seen[req.params.id] = req.json.by
    return "ok"
  },
  { log: "lazy" },
)

// The notes, for whoever asks by hand. Not recorded.
export const report = action(() => notes.list({ limit: 50 }), { log: "unrecorded" })
