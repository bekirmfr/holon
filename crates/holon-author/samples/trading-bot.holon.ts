// Trading bot: a market's ticker over a WebSocket, orders to an exchange.
//
// A market's ticker streams over a WebSocket, as Coinbase Exchange's public feed does: each price,
// {"type":"ticker","product_id":"BTC-USD","price":"64250.17","sequence":41,…}, runs the handler,
// one at a time. While runs are slower than the prices, the binding keeps only each product's
// newest (--on-full latest --key product_id), so no run acts on a stale price, and e.replaced
// says how many it replaced. The handler is lazy: its record is synced within a second, and after
// a crash the prices of its last second may run again.
//
// A level, set at PUT /levels/:product, is a price to buy at and how much, with the order's
// client id, made once, when the level is set. A price at or under the level starts the order's
// run, an action, which is strict: it signs the order with the exchange's secret (HMAC-SHA256)
// and places it. A price a crash runs again, or the action run again by hand, sends the same
// order with the same client id: the exchange places it once. An order worth more than 1000 waits
// for a person's approval first, in a run of its own: GET /orders lists its link. The action
// reconcile, run by hand, reads the exchange's open orders back, as after a restart.
//
// Before it works, an admin binds the feed:
//   HOLON_SECRET_TICKER_URL=wss://ws-feed.exchange.coinbase.com
//   holon feed add ticker --url env:HOLON_SECRET_TICKER_URL \
//     --subscribe '{"type":"subscribe","channels":["ticker"],"product_ids":["BTC-USD"]}' \
//     --only type=ticker --id product_id,sequence --on-full latest --key product_id
// and sets its secrets under the deployment's settings (the gear): exchange_key,
// exchange_secret and exchange_passphrase, an API key of the exchange's.
//
// Try it: deploy it, then
//   curl -X PUT -d '{"price":"64000","size":"0.001"}' http://127.0.0.1:8190/levels/BTC-USD
//   curl http://127.0.0.1:8190/prices      each product's last price, and how many it replaced
//   curl http://127.0.0.1:8190/orders      the orders placed, and those waiting for approval
import {
  action,
  bytes_to_text,
  decimal_compare,
  decimal_mul,
  event,
  flow,
  hmac_sha256,
  on,
  route,
  slice,
  text_to_bytes,
} from "holon"
import levels from "collection:levels"
import orders from "collection:orders"
import prices from "collection:prices"
import waiting from "collection:waiting"
import ticker from "feed:ticker"
import exchange from "host:api.exchange.coinbase.com"
import { now } from "holon:clock"
import { random_bytes } from "holon:random"
import exchange_key from "secret:exchange_key"
import exchange_passphrase from "secret:exchange_passphrase"
import exchange_secret from "secret:exchange_secret"

flow("trading-bot", { port: 8190 })

// A client order id: a UUID, from 32 random hex digits.
function uuid(hex) {
  return `${slice(hex, 0, 8)}-${slice(hex, 8, 4)}-4${slice(hex, 13, 3)}-a${slice(hex, 17, 3)}-${slice(hex, 20, 12)}`
}

// The headers that sign a request to the exchange, as Coinbase Exchange asks: an HMAC-SHA256 of
// the time, the method, the path and the body, with the secret.
function signed(method, path, body, key, secret, passphrase, at) {
  const signing = text_to_bytes(secret, { encoding: "base64" })
  const signature = hmac_sha256(signing, `${at}${method}${path}${body}`, { encoding: "base64" })
  const headers = {
    "content-type": "application/json",
    "cb-access-key": key,
    "cb-access-passphrase": passphrase,
    "cb-access-sign": signature,
    "cb-access-timestamp": `${at}`,
  }
  return headers
}

// Each price: kept by its product; at or under the product's level, the level's order, once: the
// level says when its order was started. The order's run writes nothing the handler reads, so
// neither waits for the other's keys.
on.feed(
  ticker,
  (e) => {
    const product = e.data.product_id
    prices[product] = { price: e.data.price, at: now, replaced: e.replaced }
    const level = levels[product]
    const started = level.started ?? false
    const under = decimal_compare(e.data.price, level.price) <= 0 && started === false
    const order = under
      ? { product, price: level.price, size: level.size, id: level.id }
      : undefined
    if (order !== undefined) levels[product] = { ...level, started: now }
    // Placed at once, or, worth more than 1000, asked for first: a run starts only for an order
    // that is there.
    const worth = decimal_mul(level.price, level.size)
    const large = decimal_compare(worth, "1000") > 0
    place(!large ? order : undefined)
    const asked = approval(large ? order : undefined)
    waiting[level.id] = { product, worth, approve: `/approve/${asked.waiting.approved}` }
  },
  { log: "lazy" },
)

// An order waiting for a person's approval, an hour at most; once approved, placed by a run of its
// own, which signs it with the time it is sent, and answered with that run's answer. The approval's
// body says who gave it. An hour with no approval ends the wait with { expired: true } in the
// approval's place: the order is placed by nobody, and no longer waits.
export const approval = action(async (order) => {
  const approved = await event("approved", { expires: "1h" })
  if (approved.expired) {
    waiting.delete(order.id)
    return { expired: order.id }
  }
  return place({ ...order, approved_by: JSON.parse(approved.body).by ?? "someone" })
})

// The order's run, strict: the order signed and placed, its client id the level's. The exchange
// refuses an id it has seen, so a second run for the same level places nothing.
export const place = action(async (order) => {
  const body = JSON.stringify({
    type: "limit",
    side: "buy",
    product_id: order.product,
    price: order.price,
    size: order.size,
    client_oid: order.id,
  })
  const sent = await exchange.post("/orders", {
    headers: signed(
      "POST",
      "/orders",
      body,
      exchange_key,
      exchange_secret,
      exchange_passphrase,
      now,
    ),
    body,
  })
  if (sent.status === 200) {
    orders[order.id] = {
      product: order.product,
      price: order.price,
      size: order.size,
      exchange: JSON.parse(sent.body).id,
      approved_by: order.approved_by ?? "under the limit",
    }
    waiting.delete(order.id)
  }
  return { status: sent.status, said: sent.body }
})

// The exchange's open orders, read back: what it holds, whatever the flow recorded.
export const reconcile = action(async () => {
  const open = await exchange.get("/orders", {
    headers: signed("GET", "/orders", "", exchange_key, exchange_secret, exchange_passphrase, now),
  })
  return JSON.parse(open.body)
})

// A level for a product: the price to buy at and how much, and the order's client id, made now.
route.put("/levels/:product", (req) => {
  const id = uuid(bytes_to_text(random_bytes, { encoding: "hex" }))
  levels[req.params.product] = { price: req.json.price, size: req.json.size, id }
  return { product: req.params.product, price: req.json.price, size: req.json.size, id }
})

// Each product's last price, and how many newer ones replaced the one before it waited.
route.get("/prices", () => prices.list())

// The orders placed, and those waiting for a person's approval, with the link that gives it.
route.get("/orders", () => ({ placed: orders.list(), waiting: waiting.list() }))

// Posting to an order's link approves it ({"by":"ops"}): its run places it.
route.post("/approve/:token", () => "approved", { resumes: "approved" })
