// Statement archive: zip and gzip both ways, and bytes written to a file.
//
// A bank delivers statements into a folder: a zip of camt.053 files, or one statement gzipped.
// Each delivery is unzipped or gunzipped, each statement's account, date and closing balance are
// kept, read from its XML, and the delivery is archived as one zip: its statements, and beside
// them a manifest of each one's account, closing balance and SHA-256, written as bytes. A
// statement is served back from the archive, gzipped when the client takes it. A delivery that
// isn't one is kept as refused.
//
// Before it works, an admin binds the folders:
//   holon folder add inbox ~/holon-statements --read-only
//   holon folder add archive ~/holon-archive
//
// Try it: deploy it, zip a day's camt.053 statements and drop the zip in the inbox
//   zip 2026-11-06.zip *.xml && cp 2026-11-06.zip ~/holon-statements/
// then
//   curl http://127.0.0.1:8183/deliveries                    each delivery's statements
//   curl --compressed http://127.0.0.1:8183/deliveries/2026-11-06.zip/<a file>.xml
//                                                            a statement, from the archive
//   unzip -l ~/holon-archive/2026-11-06.zip                  the archive, with its manifest
import {
  bytes_to_text,
  csv_format,
  flow,
  gunzip,
  gzip,
  on,
  respond,
  route,
  sha256,
  unzip,
  xml_attribute,
  xml_find,
  xml_parse,
  xml_text,
  zip,
} from "holon"
import deliveries from "collection:deliveries"
import archive from "folder:archive"
import inbox from "folder:inbox"

flow("statement-archive", { port: 8183 })

// A camt.053 statement's account, the date and amount of its closing balance (CLBD, negative when
// a debit), and its SHA-256.
function summary(data: Uint8Array) {
  const statement = xml_find(xml_parse(bytes_to_text(data)), "BkToCstmrStmt/Stmt")[0]
  const balance = xml_find(statement, "Bal").find((b) => xml_text(b, "Tp/CdOrPrtry/Cd") === "CLBD")
  return {
    account: xml_text(statement, "Acct/Id/IBAN"),
    date: xml_text(balance, "Dt/Dt"),
    closing: (xml_text(balance, "CdtDbtInd") === "DBIT" ? "-" : "") + xml_text(balance, "Amt"),
    currency: xml_attribute(xml_find(balance, "Amt")[0], "Ccy"),
    sha256: sha256(data),
  }
}

// The archive's name for a delivery: its name without .zip or .gz, as a zip.
function archived(name: string) {
  return (name.endsWith(".gz") ? name.slice(0, -3) : name.slice(0, -4)) + ".zip"
}

// A delivery: its statements, unzipped or gunzipped, summarized and archived as one zip.
on.file.created(inbox, (f) => {
  try {
    const delivered = inbox.bytes(f.name)
    const files = f.name.endsWith(".gz")
      ? { [f.name.slice(0, -3)]: gunzip(delivered) }
      : unzip(delivered)
    const statements = Object.keys(files).map((name) => ({ file: name, ...summary(files[name]) }))
    archive[archived(f.name)] = zip({
      ...files,
      "manifest.csv": csv_format(statements, [
        "file",
        "account",
        "date",
        "closing",
        "currency",
        "sha256",
      ]),
    })
    deliveries[f.name] = { archive: archived(f.name), statements }
  } catch {
    deliveries[f.name] = { refused: "not a zip of camt.053 statements, or one gzipped" }
  }
})

route.get("/deliveries", () => deliveries.list())

// A statement from its delivery's archive, gzipped when the client takes it.
route.get("/deliveries/:name/:file", (req) => {
  const statement = unzip(archive.bytes(archived(req.params.name)))[req.params.file]
  const gzipped = (req.headers["accept-encoding"] ?? "").includes("gzip")
  return respond(gzipped ? gzip(statement) : statement, {
    headers: gzipped
      ? { "content-type": "application/xml", "content-encoding": "gzip", vary: "accept-encoding" }
      : { "content-type": "application/xml", vary: "accept-encoding" },
  })
})
