// Run witness: keeps the run heads servers send it.
//
// Takes the run heads a holon server sends it (holon serve --export-heads), with the witness
// key. It keeps each batch with when it arrived and answers a receipt, so a run log changed
// afterwards no longer matches what was witnessed. A request without the key gets nothing kept,
// and a 401.
//
// Before it works, set its secret in the flow's Settings:
//   witness_key   the token servers send (their HOLON_EXPORT_HEADS_TOKEN)
//
// Try it: deploy it, then start a server that sends it heads, with the witness key as its token:
//   HOLON_EXPORT_HEADS_TOKEN=<witness key> holon serve --export-heads http://127.0.0.1:8130/heads
// let any deployment run (heads go out every five seconds), then
//   curl http://127.0.0.1:8130/batches
// lists the batches it kept.
import { flow, route, secure_equals, status } from "holon"
import witness_key from "secret:witness_key"
import batches from "collection:batches"
import { now } from "holon:clock"

flow("witness", { port: 8130 })

// Keeps a batch of heads with when it arrived, and answers a receipt, only with the witness key.
route.post("/heads", (req) => {
  const ok = secure_equals(req.headers.authorization, `Bearer ${witness_key}`)
  const batch = req.json.batch
  const t = now
  batches[batch] = ok ? { ...req.json, witnessed_at: t } : undefined
  return (ok ? { batch, witnessed_at: t } : undefined) ?? status(401)
})

// The kept batches, 200 at a time in key order; ?after= with the last one gives the next page.
route.get("/batches", (req) => batches.list({ after: req.query.after ?? "", limit: 200 }))
