// Token-guarded API: answers only callers holding the token.
//
// Answers a caller that sends the API token with a receipt signed by a key from the server's
// environment. The token is a secret the server seals, versioned: set a new one to rotate it, and
// roll back to an earlier one in its Settings (a dry run says what it would do first). The receipt
// key is an optional permission: without it the route still answers, its receipt "unsigned".
//
// Before it works, set its two secrets in the flow's Settings:
//   api_token     the bearer token callers send
//   receipt_key   the key receipts are signed with: choose env, then HOLON_SECRET_RECEIPT_KEY,
//                 a variable in the server's environment or its dotenv file
//
// Try it: deploy it, then
//   curl -H "Authorization: Bearer <api_token>" http://127.0.0.1:8109/me      a receipt
// Without the header, or after the token is rotated: 401. Revoke receipt_key and it answers
// "unsigned".
import { flow, hmac_sha256, route, secure_equals, status } from "holon"
import api_token from "secret:api_token"
import receipt_key from "secret:receipt_key" with { optional: "true" }

flow("guarded-api", { port: 8109 })

// Checks the bearer token, then answers a receipt signed over the path (401 without the token).
route.get("/me", (req) => {
  const ok = secure_equals(req.headers.authorization, `Bearer ${api_token}`)
  const receipt = hmac_sha256(receipt_key, req.path) ?? "unsigned"
  return (ok ? { receipt } : undefined) ?? status(401)
})
