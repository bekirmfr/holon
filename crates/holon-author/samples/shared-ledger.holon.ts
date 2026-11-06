// Shared ledger: one file, several deployments.
//
// Appends each line posted to one file in a folder on the host, and serves it. Deployed twice, two
// flows write the same file at once and every line lands: a run commits only if the file is as it
// read it, and the check and the write hold the file's commit lock, which every deployment's runs
// share. A run that finds the file changed runs again on the file as it now is.
//
// Before it works, bind the folder (an admin):
//   holon folder add ledger ~/holon-ledger
//
// Try it: deploy it, then deploy a copy renamed shared-ledger-2 that serves 127.0.0.1:8141. Post
// to both at once:
//   for i in $(seq 20); do curl -s -X POST -d "a $i" http://127.0.0.1:8140/lines &
//     curl -s -X POST -d "b $i" http://127.0.0.1:8141/lines & done; wait
//   curl http://127.0.0.1:8140/lines       all 40 lines
import { flow, respond, route } from "holon"
import ledger from "folder:ledger"

flow("shared-ledger", { port: 8140 })

// Appends the posted body to the file as one line.
route.post("/lines", (req) => {
  const before = ledger["lines.txt"] ?? ""
  ledger["lines.txt"] = `${before}${req.body}
`
  return { added: req.body }
})

// The whole file, as plain text.
route.get("/lines", () =>
  respond(ledger["lines.txt"], { headers: { "content-type": "text/plain; charset=utf-8" } }),
)
