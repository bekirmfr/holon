// Idempotent charges: one call per key.
//
// Charges once per Idempotency-Key. The same key with the same body answers the first charge
// again, the same key with a different body is refused (409), and a key expires after a day. The
// processor (httpbin.org, standing in) is called only when there is no prior charge. Two identical
// requests at once: the second waits for the first's claim on the key, reads what it wrote, and
// sends nothing.
//
// Try it: deploy it (holon deploy idempotent.holon.ts), then send this twice:
//   curl -X POST -H 'Idempotency-Key: k1' -H 'Content-Type: application/json' \
//     -d '{"amount":42}' http://127.0.0.1:8121/charges
// The same id both times, the second with replayed: true. Send it with amount 43 for a 409. Run two
// at once with a fresh key, k2, and explain (holon mcp) shows the second claimed charges[k2],
// waited, and sent nothing.
import { flow, route, sha256, slice, status } from "holon"
import charges from "collection:charges"
import httpbin from "host:httpbin.org"
import { now } from "holon:clock"
import { random } from "holon:random"

flow("idempotent", { port: 8121 })

// Charges once per key: answers a prior charge again, refuses a different body (409), or charges.
route.post("/charges", async (req) => {
  const key = req.headers["idempotency-key"]
  const fingerprint = sha256(req.body)
  const stored = charges[key]
  const prior = stored.expires_at > now ? stored : undefined
  const replayed = (prior.fingerprint ?? "") === fingerprint
  const mismatch = (prior.fingerprint ?? fingerprint) !== fingerprint
  // The charge is sent only when there is no prior one: once sent it can't be taken back, so the
  // request says when it goes out.
  const first_charge = !replayed && !mismatch ? true : undefined
  let processed
  if (first_charge !== undefined) {
    processed = await httpbin.post("/post", {
      headers: { "content-type": "application/json" },
      body: req.body,
    })
  }
  const fresh = {
    id: slice(sha256(`${key}:${now}:${random}`), 0, 12),
    amount: req.json.amount,
    currency: req.json.currency ?? "EUR",
    processor_saw: JSON.parse(processed.body).json,
    fingerprint,
    charged_at: now,
    expires_at: now + 86400,
  }
  const result = !mismatch ? (prior ?? fresh) : undefined
  charges[key] = result
  return { ...result, replayed } ?? status(409)
})

// The charge kept under a key.
route.get("/charges/:key", (req) => charges[req.params.key])
