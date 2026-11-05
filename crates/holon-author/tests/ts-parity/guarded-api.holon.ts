// Guarded API (samples/guarded-api.holon) in TypeScript's syntax: the same unit.
import { flow, hmac_sha256, route, secure_equals, status } from "holon"
import api_token from "secret:api_token"
import receipt_key from "secret:receipt_key" with { optional: "true" }

flow("guarded-api", { port: 8109 })

// Answers the caller's receipt when it sends the token, 401 when not.
route.get("/me", (req) => {
  const ok = secure_equals(req.headers.authorization, `Bearer ${api_token}`)
  const receipt = hmac_sha256(receipt_key, req.path) ?? "unsigned"
  return (ok ? { receipt } : undefined) ?? status(401)
})
