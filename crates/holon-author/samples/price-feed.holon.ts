// Price feed: a feed of prices, each event a run.
//
// A market data provider streams prices as server-sent events. The host keeps the connection, and
// each event the feed sends starts one run of the handler, one at a time, in the order they came:
// e is the event, its data a value since it is JSON. The price is kept per symbol, sent on a
// channel the page listens to, and answered at GET /prices. The host keeps where the feed is: a
// connection lost is opened again from the last event run on (Last-Event-ID), and an event the
// feed sends again is passed over. The flow never sees the feed's URL or its key.
//
// Before it works, an admin binds the feed, its URL in a secret source:
//   HOLON_SECRET_PRICES_URL=https://stream.example.com/v1/prices?key=…
//   holon feed add prices --url env:HOLON_SECRET_PRICES_URL
//
// Try it: deploy it, open http://127.0.0.1:8187/ and watch each price come, or
//   curl http://127.0.0.1:8187/prices         {"ACME": {"price": 10.5, "at": …}, …}
import { flow, js, on, respond, route } from "holon"
import latest from "collection:latest"
import ticks from "channel:ticks"
import prices from "feed:prices"
import { now } from "holon:clock"

flow("price-feed", { port: 8187 })

// The page: the last price of each symbol, and each new one as it comes.
const page = js(function html(board) {
  const esc = (s) =>
    String(s).replace(
      /[&<>"']/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
    )
  const rows = Object.entries(board).map(
    ([symbol, p]) => `<tr id="${esc(symbol)}"><td>${esc(symbol)}</td><td>${esc(p.price)}</td></tr>`,
  )
  return `<!doctype html>
<meta charset="utf-8">
<title>Price feed</title>
<style>body{font:16px/1.5 system-ui;max-width:30em;margin:3em auto;padding:0 1em}td{padding:0 1em}tr.new{animation:in 1s}@keyframes in{from{background:#ffd}}</style>
<h1>Prices</h1>
<table id="prices">${rows.join("\n")}</table>
<script>
const table = document.getElementById("prices");
new EventSource("/live").onmessage = (e) => {
  const p = JSON.parse(e.data);
  let row = document.getElementById(p.symbol);
  if (!row) {
    row = table.insertRow();
    row.id = p.symbol;
    row.insertCell().textContent = p.symbol;
    row.insertCell();
  }
  row.cells[1].textContent = p.price;
  row.className = "";
  void row.offsetWidth;
  row.className = "new";
};
</script>`
})

// Each event, {"symbol": "ACME", "price": 10.5}: kept by its symbol, and told to the page.
on.feed(prices, (e) => {
  const symbol = e.data.symbol
  latest[symbol] = { price: e.data.price, at: now }
  ticks.send({ symbol, price: e.data.price, at: now })
})

route.get("/", () =>
  respond(page(latest.list()), { headers: { "content-type": "text/html; charset=utf-8" } }),
)

// The last price of each symbol.
route.get("/prices", () => latest.list())

// Each new price as server-sent events, for as long as the caller stays.
route.get("/live", () => ticks.subscribe())
