// Ticker (samples/ticker.holon) in TypeScript's syntax: the same unit.
import { action, every, flow, js, respond, route } from "holon"
import { now } from "holon:clock"
import state from "state"
import ticks from "channel:ticks"

flow("ticker", { port: 8156 })

const page = js(function html(count) {
  return `<!doctype html>
<meta charset="utf-8">
<title>Ticker</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}li.new{animation:in 1s}@keyframes in{from{background:#ffd}}</style>
<h1>Ticker</h1>
<p>${count} ticks so far; each new one shows below as it arrives.</p>
<ul id="ticks"></ul>
<script>
const list = document.getElementById("ticks");
new EventSource("/live").onmessage = (e) => {
  const tick = JSON.parse(e.data);
  const li = document.createElement("li");
  li.className = "new";
  li.textContent = "tick " + tick.n + " at " + new Date(tick.at * 1000).toLocaleTimeString();
  list.prepend(li);
};
</script>`;
})

// Every two seconds, one more tick, sent to whoever listens.
every("2s", () => {
  const n = (state.ticks ?? 0) + 1
  state.ticks = n
  ticks.send({ n, at: now })
})

// Sets the count, and answers what it was.
export const reset = action((input) => {
  state.ticks = input.to ?? 0
  return { was: state.ticks ?? 0, set: input.to ?? 0 }
})

route.get("/", () => respond(page(state.ticks ?? 0), { headers: { "content-type": "text/html; charset=utf-8" } }))

route.get("/count", () => state.ticks ?? 0)

route.get("/live", () => ticks.subscribe())
