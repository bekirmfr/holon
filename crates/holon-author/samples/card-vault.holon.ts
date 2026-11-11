// Card vault: a merchant keeps a card and gets a token for it; the number never reaches the log.
//
// A merchant's server posts a card with its key in x-merchant-key. The route checks the key, keeps
// the card under a token signed from its number (its last four digits and its expiry, never the
// number), and answers the token. The flow taints the key and the card's number wherever its runs
// have them, a request, a WebSocket's message, a call's request or answer, an action's input, a
// row it writes or reads: its runs' log, lines, export and a replay show ‹taint:number› where the
// number was, as every flow's show ‹taint:authorization› for a bearer token and ‹taint:cookie› for
// a cookie. The caller is answered as always.
//
// Before it works, set its secret under the deployment's settings (the gear):
//   merchant_key   the key merchants send, six characters or more
//
// Try it: deploy it, then
//   curl -H "x-merchant-key: <merchant_key>" -d '{"number":"4111111111111111","expiry":"12/30"}'
//     http://127.0.0.1:8200/cards
//   open the run: its request shows ‹taint:x-merchant-key› and ‹taint:number› in their place
import { flow, hmac_sha256, route, secure_equals, status } from "holon"
import cards from "collection:cards"
import merchant_key from "secret:merchant_key"

flow("card-vault", { port: 8200, taint: ["x-merchant-key", "number"] })

// Keeps a card under a token, with the merchant's key (401 without it).
route.post("/cards", (req) => {
  const token = `card_${hmac_sha256(merchant_key, req.json.number)}`
  const card = secure_equals(req.headers["x-merchant-key"], merchant_key)
    ? { last4: req.json.number.slice(-4), expiry: req.json.expiry }
    : undefined
  cards[token] = card
  return (card ? { token, last4: card.last4 } : undefined) ?? status(401)
})
