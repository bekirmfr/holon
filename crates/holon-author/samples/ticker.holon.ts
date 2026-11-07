// Ticker: a clock beside routes.
//
// A flow with a clock of its own beside its routes: every 2 seconds it counts a tick in its state
// and sends it on the channel ticks, with the time of the tick. Its page lists each tick as it
// arrives, and GET /count answers the count. An action, run by hand by its name, sets the count
// back: a person runs it, and an agent only once a person lets agents run it.
//
// Try it: deploy it, open http://127.0.0.1:8156/ and a new tick shows every 2 seconds;
//   curl http://127.0.0.1:8156/count
// answers how many so far. To set it back, run reset from the editor's server panel or the
// deployed page (or POST {"to": 0} to /trigger/ticker/reset on the holon server). To let an agent
// run it (holon mcp's run tool), tick reset under the gear's "agents may run", deploy and approve.
import { action, every, flow, js, respond, route } from "holon"
import state from "state"
import ticks from "channel:ticks"
import { now } from "holon:clock"

flow("ticker", { port: 8156 })

// The page: the count so far, and each new tick as it arrives.
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
</script>`
})

// Every 2 seconds: one more tick in the count, and sent on the channel with its time.
every("2s", () => {
  const n = (state.ticks ?? 0) + 1
  state.ticks = n
  ticks.send({ n, at: now })
})

// Run by hand: sets the count to what was sent (0 by default), and answers what it was before.
export const reset = action((input) => {
  state.ticks = input.to ?? 0
  return { was: state.ticks ?? 0, set: input.to ?? 0 }
})

// The page, with the count so far.
route.get("/", () =>
  respond(page(state.ticks ?? 0), { headers: { "content-type": "text/html; charset=utf-8" } }),
)

// How many ticks so far.
route.get("/count", () => state.ticks ?? 0)

// Each new tick as server-sent events, for the page.
route.get("/live", () => ticks.subscribe())
