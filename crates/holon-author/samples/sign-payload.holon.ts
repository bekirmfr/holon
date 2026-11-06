// Sign a payload: an HMAC with a secret key.
//
// Signs whatever is posted with a secret key (HMAC-SHA256) and answers the signature, in hex, with
// the body in base64. The key is never in the run log, yet holon verify checks each run from the
// signature its log entry kept.
//
// Before it works, set its secret under the deployment's settings (the gear):
//   signing_key   any secret of 6 characters or more to sign with
//
// Try it: deploy it, then
//   curl -X POST -H "Content-Type: application/json" -d 'hello' http://127.0.0.1:8107/sign
// then export its runs (holon export sign-payload) and run holon verify on the export.
import { base64, flow, hmac_sha256, route, status } from "holon"
import signing_key from "secret:signing_key"

flow("sign-payload", { port: 8107 })

// Signs the raw body with the key, and answers the signature and the body in base64.
route.post("/sign", (req) => {
  return (
    { signature: hmac_sha256(signing_key, req.body), body_base64: base64(req.body) } ?? status(400)
  )
})
