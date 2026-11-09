// Trade tape: a market's trades over a WebSocket, the ones the feed skipped fetched.
//
// A market's trades stream over a WebSocket, as Coinbase Exchange's public matches channel sends
// them: {"type":"match","trade_id":1117440338,"product_id":"BTC-USD","size":"0.0042",…}, each
// product's trade ids one after another, and first the trade before it subscribed (last_match).
// The binding numbers each product's trades by trade_id (--sequence trade_id --key product_id), so
// each event's e.gap says how many of its product's trades came while the host wasn't reading,
// across a reconnect or a restart: the feed never sends them again. The handler adds each trade's
// size to its product's volume and, when e.gap is above 0, fetches the trades skipped from the
// exchange's public REST API, a page of 1,000 at most, and adds theirs: the volume stays exact. A
// gap past 1,000 adds what one page holds, and keeps how many it couldn't. Its runs are strict:
// where it is in the feed moves once a run's record is on disk, so a trade a crash cut short is
// in the next one's gap.
//
// Before it works, an admin binds the feed:
//   HOLON_SECRET_TRADES_URL=wss://ws-feed.exchange.coinbase.com
//   holon feed add trades --url env:HOLON_SECRET_TRADES_URL \
//     --subscribe '{"type":"subscribe","channels":["matches"],"product_ids":["BTC-USD","ETH-USD"]}' \
//     --only type=match --only type=last_match --id product_id,trade_id \
//     --sequence trade_id --key product_id
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8193/volumes      each product's volume, its trades, and those fetched
import { decimal_sum, flow, on, route } from "holon"
import volumes from "collection:volumes"
import trades from "feed:trades"
import exchange from "host:api.exchange.coinbase.com"

flow("trade-tape", { port: 8193 })

// Each trade, added to its product's volume with the trades skipped before it, fetched: a page of
// the trades before its id (after=<trade_id>), the newest first.
on.feed(trades, async (e) => {
  const trade = e.data
  const product = trade.product_id
  const was = volumes[product] ?? { volume: "0", trades: 0, fetched: 0, unfetched: 0 }
  const page =
    e.gap > 0
      ? await exchange.get(
          `/products/${product}/trades?after=${trade.trade_id}&limit=${Math.min(e.gap, 1000)}`,
        )
      : undefined
  // None asked, or none given: none added, and the gap is kept as unfetched.
  const skipped = (page.status === 200 ? JSON.parse(page.body) : undefined) ?? []
  const sizes = skipped.map((t) => t.size)
  volumes[product] = {
    volume: decimal_sum([was.volume, trade.size, ...sizes]),
    trades: was.trades + 1 + sizes.length,
    fetched: was.fetched + sizes.length,
    unfetched: was.unfetched + e.gap - sizes.length,
    last: trade.trade_id,
  }
})

// Each product's volume: its trades, those fetched, and those a page couldn't hold.
route.get("/volumes", () => volumes.list())
