// Journal: a folder kept for a while, and capped.
//
// Appends each line posted to a page, a text file in a folder on the host, and serves the page.
// Every run reads the page whole, so the run log keeps what it read: past 4 KiB in the blob store,
// by content and in chunks, so a page that grows by a line adds a chunk, not a copy. The admin
// decides how long those texts are kept and how big the folder may get: a write past the cap
// doesn't land, and the run answers 500 saying so.
//
// Before it works, an admin binds the folder, its texts kept for 7 days (then only their hash)
// and the folder capped at 10 MB:
//   holon folder add journal ~/holon-journal --keep 7d --cap 10M
//
// Try it: deploy it, then
//   curl -s -X POST -d 'first line' http://127.0.0.1:8125/journal/today
//   curl -s -X POST -d 'second line' http://127.0.0.1:8125/journal/today
//   curl http://127.0.0.1:8125/journal/today            both lines
//   curl http://127.0.0.1:8125/journal                  the pages
// holon folder sweep then removes the texts that no run kept within 7 days refers to.
import { flow, respond, route } from "holon"
import journal from "folder:journal"

flow("journal", { port: 8125 })

// Adds the posted text as a line at the end of the page's file.
route.post("/journal/:page", (req) => {
  const name = `${req.params.page}.txt`
  const before = journal[name] ?? ""
  journal[name] = `${before}${req.body}
`
  return { page: name, added: req.body }
})

// Serves a page as plain text.
route.get("/journal/:page", (req) =>
  respond(journal[`${req.params.page}.txt`], {
    headers: { "content-type": "text/plain; charset=utf-8" },
  }),
)

// Lists the pages, with their size, hash and time.
route.get("/journal", () => journal.list())
