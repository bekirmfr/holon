// Drop box: a run per new file, sent on by handle.
//
// Runs once for each new file dropped into the folder drop, when it stops changing: the handler
// is given the file's handle (name, size, hash). It sends the file to httpbin.org as the body of
// a PUT, by handle: the server streams the bytes from the folder, never through the flow, and the
// run log keeps only their hash. It writes a receipt for each file to the folder receipts. Files
// already there when it is deployed aren't new; one it writes into a folder it watches doesn't
// start it again.
//
// Before it works, bind the two folders (an admin):
//   holon folder add drop ~/holon-drop --read-only
//   holon folder add receipts ~/holon-receipts
//
// Try it: deploy it, then copy any file into ~/holon-drop. Within two seconds ~/holon-receipts
// holds <file>.json with its size, its hash and how many bytes httpbin.org received, and the
// deployment's log shows the run.
import { flow, on } from "holon"
import receipts from "folder:receipts"
import drop from "folder:drop"
import httpbin from "host:httpbin.org"

flow("drop-box")

// Each new file: sent on by its handle, and a receipt written for it.
on.file.created(drop, async (f) => {
  // Sends the file to httpbin.org as a PUT's body, streamed from the folder by its handle.
  const sent = await httpbin.put("/put", {
    headers: { "content-type": "application/octet-stream" },
    file: f,
  })
  // Writes the file's receipt: its name, size and hash, the status, and the bytes httpbin.org got.
  receipts[`${f.name}.json`] = JSON.stringify({
    file: f.name,
    size: f.size,
    hash: f.hash,
    status: sent.status,
    received: JSON.parse(sent.body).headers["Content-Length"] ?? "nothing",
  })
})
