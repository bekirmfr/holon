// JWT API: partners call with tokens they sign themselves, checked with keys a person trusts.
//
// A payouts API for partners' services. Each partner signs its own tokens (JWTs, as OAuth's
// private_key_jwt does) with a key only it holds; the API keeps each partner's public key, which
// a person adds by running the trust action, so an agent can't add a partner on its own. A
// request's bearer token is read for who says it signed it (jwt_decode), then verified with that
// partner's key, for this API, at the time of the run (jwt_verify): its signature, its expiry and
// its audience. The key decides the algorithm (RS256, PS256, ES256 or EdDSA), so a token naming
// none, or signed with HMAC over a public key, is refused. A payout needs the payouts:write scope,
// and is kept under the token's id, so a token sent twice pays once.
//
// Try it: deploy it, run trust from the editor's server panel with a partner's public key, a JWK
// or a PEM text: {"partner": "acme", "key": {"kty": "EC", "crv": "P-256", "x": "…", "y": "…"}}.
// Then, with a token acme signed (iss acme, aud payouts-api, an exp, a jti):
//   curl -H "authorization: Bearer $TOKEN" http://127.0.0.1:8184/whoami
//   curl -H "authorization: Bearer $TOKEN" -d '{"amount": "120.00"}' http://127.0.0.1:8184/payouts
import { action, flow, jwt_decode, jwt_verify, respond, route } from "holon"
import { now } from "holon:clock"
import partners from "collection:partners"
import payouts from "collection:payouts"

flow("jwt-api", { port: 8184 })

// A partner's public key, trusted by a person: a JWK or a PEM text, under the partner's id.
export const trust = action((input) => {
  partners[input.partner] = input.key
})

// The claims of a request's bearer token, verified with the key of the partner it names, for
// this API, now; nothing when it doesn't verify.
function caller(authorization: string) {
  const token = authorization.replace("Bearer ", "")
  return jwt_verify(token, partners[jwt_decode(token).payload.iss], now, {
    audience: "payouts-api",
  })
}

const unauthorized = "a token a trusted partner signed for payouts-api is needed"

// Who called.
route.get("/whoami", (req) => {
  try {
    const claims = caller(req.headers.authorization ?? "")
    return { partner: claims.iss, subject: claims.sub, scope: claims.scope, expires: claims.exp }
  } catch {
    return respond(
      { error: unauthorized },
      { status: 401, headers: { "www-authenticate": "Bearer" } },
    )
  }
})

// A payout, asked for with the payouts:write scope; kept under the token's id (jti), once.
route.post("/payouts", (req) => {
  try {
    const claims = caller(req.headers.authorization ?? "")
    const id = `${claims.iss}/${claims.jti}`
    if (!(claims.scope ?? "").split(" ").includes("payouts:write")) {
      return respond({ error: "the token's scope has no payouts:write" }, { status: 403 })
    }
    const kept = payouts[id] ?? false
    if (kept === false) {
      payouts[id] = { partner: claims.iss, by: claims.sub, amount: req.json.amount ?? "", at: now }
    }
    return respond(
      kept === false
        ? { accepted: id }
        : { error: "this token has asked for a payout already", payout: id },
      { status: kept === false ? 202 : 409 },
    )
  } catch {
    return respond(
      { error: unauthorized },
      { status: 401, headers: { "www-authenticate": "Bearer" } },
    )
  }
})
