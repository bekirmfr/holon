// File checksums: each file's SHA-256, over its bytes.
//
// Runs once for each file new or changed in the folder drop, and reads its bytes: any file, an
// image, a PDF or an archive as well as a text. It keeps the file's size, its SHA-256 (what
// sha256sum prints for it) and what its first bytes say it is, under its name. A route reads one
// back, another lists them all. The run log keeps a small file's bytes and a big one's hash, so
// holon verify checks each run from them.
//
// Before it works, bind the folder (an admin):
//   holon folder add drop ~/holon-drop --read-only
//
// Try it: deploy it, copy any file into ~/holon-drop, then
//   curl http://127.0.0.1:8179/sums/<file>
// for its size, its SHA-256 and its kind; GET /sums lists every file's.
import { bytes_to_text, flow, on, route, sha256 } from "holon"
import sums from "collection:sums"
import drop from "folder:drop"

flow("file-checksums", { port: 8179 })

// Each file new or changed: its bytes, hashed, and its first four bytes, in hex, read as a kind.
on.file.created(drop, (f) => {
  const b = drop.bytes(f.name)
  const magic = bytes_to_text(b.slice(0, 4), { encoding: "hex" })
  const kind =
    magic === "89504e47"
      ? "png"
      : magic === "25504446"
        ? "pdf"
        : magic === "504b0304"
          ? "zip"
          : "other"
  sums[f.name] = { size: b.length, sha256: sha256(b), kind }
})

// One file's, by its name.
route.get("/sums/:name", (req) => sums[req.params.name])

// Every file's.
route.get("/sums", () => sums.list())
