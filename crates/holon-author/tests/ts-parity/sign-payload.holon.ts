// Sign a payload (samples/sign-payload.holon) in TypeScript's syntax: the same unit.
import { base64, flow, hmac_sha256, route, status } from "holon"
import signing_key from "secret:signing_key"

flow("sign-payload", { port: 8107 })

// Signs the raw body with the key, and answers the signature and the body in base64.
route.post("/sign", ({ body }) => {
  return { signature: hmac_sha256(signing_key, body), body_base64: base64(body) } ?? status(400)
})
