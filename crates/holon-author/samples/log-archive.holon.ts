// Log archive: big files, by handle and by range.
//
// Serves the last lines and the first bytes of a log of any size, the whole log as a download, and
// archives a log to another folder, host to host. Only the lines or bytes asked for enter the
// flow, and the run log keeps with them the hashes that prove them part of the file, so holon
// verify checks the run without the file. A download is the log's handle, served: the host
// streams its bytes, and cuts the connection if the log changes while it is sent. Archiving reads
// a handle (name, size, hash), never the bytes: the copy streams them when the run finishes, and
// fails if the log changed since it was read.
//
// Before it works, an admin binds the folders:
//   holon folder add logs ~/holon-logs --read-only
//   holon folder add archive ~/holon-archive
// then put any app.log in ~/holon-logs.
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8124/logs/app.log/tail       its last 20 lines
//   curl http://127.0.0.1:8124/logs/app.log/head       its first 256 bytes
//   curl -OJ http://127.0.0.1:8124/logs/app.log/download  all of it, saved as app.log
//   curl -s -X POST -H 'content-type: application/json' -d '{"name":"app.log"}' \
//     http://127.0.0.1:8124/archive                    copies it into the archive under its hash
//   curl http://127.0.0.1:8124/archive                 lists the archive
import { flow, respond, route } from "holon"
import archive from "folder:archive"
import logs from "folder:logs"

flow("log-archive", { port: 8124 })

// A log's last 20 lines.
route.get("/logs/:name/tail", (req) => logs.lines(req.params.name, { from: -20, limit: 20 }))

// A log's first 256 bytes.
route.get("/logs/:name/head", (req) => logs.slice(req.params.name, { offset: 0, length: 256 }))

// A whole log, as a download.
route.get("/logs/:name/download", (req) =>
  respond(logs.serve(req.params.name), {
    headers: { "content-disposition": `attachment; filename="${req.params.name}"` },
  }),
)

// Copies a log into the archive, by handle, named by its name and hash.
route.post("/archive", (req) => {
  const log = logs.file(req.json.name)
  archive[`${log.name}.${log.hash}`] = log
  return { archived: `${log.name}.${log.hash}`, size: log.size }
})

// Lists what is archived.
route.get("/archive", () => archive.list())
