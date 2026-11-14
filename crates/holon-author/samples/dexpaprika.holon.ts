// DexPaprika: an n8n community node, translated into Holon text.
//
// n8n-nodes-dexpaprika (CoinPaprika, MIT) is a declarative n8n node for DEX and on-chain market
// data across 36 blockchains, free without a key. Each of its thirteen operations is a holon here,
// and a route calls each so it can be tried; none of the node's JavaScript runs. How each piece
// maps is in docs/L4s/n8n-import-L4.md:
//   requestDefaults.baseURL      the host, a literal: api.dexpaprika.com is a permission asked for
//   an operation's own baseURL   a second host, api-pro.dexpaprika.com (paid plans), asked for apart
//   the optional credential      optional secret dexpaprika_key, the whole authorization header; the
//                                approval says it is sent to those two hosts and nowhere else
//   ={{$parameter.network}}      the holon's input, interpolated into the path
//   send: { type: 'query' }      the caller's parameters over the node's defaults, made a query
//                                string by a js holon (Holon has no leaf that escapes a URL)
//   preSend assertPathParameter  the holon segment: an id that would add to the path is refused
//
// Optional: set dexpaprika_key in the flow's Settings for a higher rate limit
// and the two pro operations (token ohlcv and indicators); without it the free tier answers.
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8172/networks
//   curl "http://127.0.0.1:8172/search?query=weth"
//   curl "http://127.0.0.1:8172/networks/ethereum/pools?limit=3"
// Each answers DexPaprika's status and JSON. An id with / \ ? # or % in it answers 400, and nothing
// is sent.
import { flow, js, route, status } from "holon"
import dexpaprika_key from "secret:dexpaprika_key" with { optional: "true" }
import dexpaprika_com from "host:api.dexpaprika.com"
import api_pro from "host:api-pro.dexpaprika.com"

flow("dexpaprika", { port: 8172 })

// requestDefaults and the credential: a GET to the host with JSON accepted, and the key as the
// whole authorization value when one is set. Answers the API's status and JSON (its text when it
// isn't JSON; 502 and why when the call got no answer).
async function dexpaprika(path) {
  const response = await dexpaprika_com.get(path, {
    headers: { accept: "application/json", authorization: dexpaprika_key } ?? {
      accept: "application/json",
    },
  })
  const answer = {
    status: response.status === 0 ? 502 : response.status,
    body: JSON.parse(response.body) ??
      response.body ?? { error: response.error, message: response.message },
  }
  return answer
}

// Network: List Networks. Each operation is a holon, its parameters its inputs.
async function networks() {
  const answer = await dexpaprika("/networks")
  return answer
}

// The node's preSend check (assertPathParameter): a network id or an address goes into the path,
// so one that would add a segment, a query or a fragment is absent, and nothing is sent. Holon
// refuses a path with whitespace or a control character itself.
function segment(value) {
  const ok =
    value !== "" &&
    value !== "." &&
    value !== ".." &&
    value.indexOf("/") < 0 &&
    value.indexOf("\\") < 0 &&
    value.indexOf("?") < 0 &&
    value.indexOf("#") < 0 &&
    value.indexOf("%") < 0
      ? value
      : undefined
  return ok
}

// Network: List DEXes.
async function dexes(network) {
  const answer = await dexpaprika(`/networks/${segment(network)}/dexes`)
  return answer
}

// Network: Get Stats.
async function stats() {
  const answer = await dexpaprika("/stats")
  return answer
}

// send: { type: 'query' }: each parameter the caller gave, else the node's default for it, escaped
// as axios would, the empty ones left out.
const query = js(function text(given, defaults) {
  const q = { ...defaults }
  for (const k of Object.keys(defaults))
    if (given[k] !== undefined && given[k] !== "") q[k] = given[k]
  return Object.entries(q)
    .filter(([, v]) => v !== "")
    .map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
    .join("&")
})

// Token: Search, across every network.
async function search(given) {
  const answer = await dexpaprika(`/search?${query(given, { query: "" })}`)
  return answer
}

// Token: Get Top Tokens.
async function top_tokens(network, given) {
  const answer = await dexpaprika(
    `/networks/${segment(network)}/tokens/search?${query(given, { limit: 50 })}`,
  )
  return answer
}

// Token: Get Details.
async function token(network, contractAddress) {
  const answer = await dexpaprika(
    `/networks/${segment(network)}/tokens/${segment(contractAddress)}`,
  )
  return answer
}

// Token: Get Prices (Batch), up to 10 comma-separated addresses.
async function prices(network, given) {
  const answer = await dexpaprika(
    `/networks/${segment(network)}/multi/prices?${query(given, { tokens: "" })}`,
  )
  return answer
}

// The same on the pro host, which two token operations name as their own baseURL.
async function dexpaprika_pro(path) {
  const response = await api_pro.get(path, {
    headers: { accept: "application/json", authorization: dexpaprika_key } ?? {
      accept: "application/json",
    },
  })
  const answer = {
    status: response.status === 0 ? 502 : response.status,
    body: JSON.parse(response.body) ??
      response.body ?? { error: response.error, message: response.message },
  }
  return answer
}

// Token: Get Token OHLCV (pro).
async function token_ohlcv(network, contractAddress, given) {
  const answer = await dexpaprika_pro(
    `/networks/${segment(network)}/tokens/${segment(contractAddress)}/ohlcv?${query(given, { start: "-24h", end: "", interval: "24h", limit: 50 })}`,
  )
  return answer
}

// Token: Get Indicator (pro): indicator and interval, then the additional fields, sent only when
// given ("" is no default).
async function token_indicator(network, contractAddress, given) {
  const answer = await dexpaprika_pro(
    `/networks/${segment(network)}/tokens/${segment(contractAddress)}/indicators?${query(given, { indicator: "rsi", interval: "", cursor: "", end: "", fast_period: "", limit: "", period: "", signal_period: "", slow_period: "", std_dev: "", start: "" })}`,
  )
  return answer
}

// Pool: Get Top Pools.
async function top_pools(network, given) {
  const answer = await dexpaprika(
    `/networks/${segment(network)}/pools/search?${query(given, { limit: 50 })}`,
  )
  return answer
}

// Pool: Get Details.
async function pool(network, poolAddress) {
  const answer = await dexpaprika(`/networks/${segment(network)}/pools/${segment(poolAddress)}`)
  return answer
}

// Pool: Get OHLCV.
async function pool_ohlcv(network, poolAddress, given) {
  const answer = await dexpaprika(
    `/networks/${segment(network)}/pools/${segment(poolAddress)}/ohlcv?${query(given, { start: "-24h", interval: "24h", limit: 50 })}`,
  )
  return answer
}

// Pool: Get Indicator, with the same fields.
async function pool_indicator(network, poolAddress, given) {
  const answer = await dexpaprika(
    `/networks/${segment(network)}/pools/${segment(poolAddress)}/indicators?${query(given, { indicator: "rsi", interval: "", cursor: "", end: "", fast_period: "", limit: "", period: "", signal_period: "", slow_period: "", std_dev: "", start: "" })}`,
  )
  return answer
}

// Each operation as a route, answered with DexPaprika's status and JSON; 400 when an id can't
// be a path segment. The networks DexPaprika covers.
route.get("/networks", async () => {
  const found = await networks()
  return status(found.status, found.body) ?? status(400)
})

// The exchanges on one network.
route.get("/networks/:network/dexes", async (req) => {
  const found = await dexes(req.params.network)
  return status(found.status, found.body) ?? status(400)
})

// Platform statistics.
route.get("/stats", async () => {
  const found = await stats()
  return status(found.status, found.body) ?? status(400)
})

// Tokens, pools and exchanges matching ?query=, across every network.
route.get("/search", async (req) => {
  const found = await search(req.query)
  return status(found.status, found.body) ?? status(400)
})

// The top tokens on a network by volume (?limit=, 50 by default).
route.get("/networks/:network/tokens", async (req) => {
  const found = await top_tokens(req.params.network, req.query)
  return status(found.status, found.body) ?? status(400)
})

// One token's price and metadata.
route.get("/networks/:network/tokens/:address", async (req) => {
  const found = await token(req.params.network, req.params.address)
  return status(found.status, found.body) ?? status(400)
})

// USD prices for ?tokens=a,b,c on one network.
route.get("/networks/:network/prices", async (req) => {
  const found = await prices(req.params.network, req.query)
  return status(found.status, found.body) ?? status(400)
})

// A token's candles (?start= &end= &interval= &limit=); a pro plan's key.
route.get("/networks/:network/tokens/:address/ohlcv", async (req) => {
  const found = await token_ohlcv(req.params.network, req.params.address, req.query)
  return status(found.status, found.body) ?? status(400)
})

// A token's indicator (?indicator= &interval= and the rest); a pro plan's key.
route.get("/networks/:network/tokens/:address/indicators", async (req) => {
  const found = await token_indicator(req.params.network, req.params.address, req.query)
  return status(found.status, found.body) ?? status(400)
})

// The top pools on a network by volume (?limit=, 50 by default).
route.get("/networks/:network/pools", async (req) => {
  const found = await top_pools(req.params.network, req.query)
  return status(found.status, found.body) ?? status(400)
})

// One pool.
route.get("/networks/:network/pools/:address", async (req) => {
  const found = await pool(req.params.network, req.params.address)
  return status(found.status, found.body) ?? status(400)
})

// A pool's candles (?start= &interval= &limit=).
route.get("/networks/:network/pools/:address/ohlcv", async (req) => {
  const found = await pool_ohlcv(req.params.network, req.params.address, req.query)
  return status(found.status, found.body) ?? status(400)
})

// A pool's indicator (?indicator= &interval= and the rest).
route.get("/networks/:network/pools/:address/indicators", async (req) => {
  const found = await pool_indicator(req.params.network, req.params.address, req.query)
  return status(found.status, found.body) ?? status(400)
})
