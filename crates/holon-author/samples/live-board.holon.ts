// Live board: a channel, packs to whoever listens.
//
// A page that updates itself. Posting a ticket stores it and sends a pack on the channel updates;
// every page open on the board listens (GET /live answers the channel's packs as server-sent
// events) and adds the ticket as it arrives. A pack goes out once its run is over and its writes
// have landed; a page that reconnects gets the packs it missed. Sending and letting callers
// subscribe are each a permission.
//
// Try it: deploy it, open http://127.0.0.1:8126/ in two browsers and add a ticket in one: it shows
// in both. Or in one terminal
//   curl -N http://127.0.0.1:8126/live
// and in another
//   curl -s -H 'content-type: application/json' -d '{"title":"fix the door"}' \
//     http://127.0.0.1:8126/tickets
// The first prints id: ... and data: {"at":...,"title":"fix the door"}.
import { as_text, flow, js, respond, route, sha256, slice, status } from "holon"
import tickets from "collection:tickets"
import updates from "channel:updates"
import { now } from "holon:clock"
import { random } from "holon:random"

flow("live-board", { port: 8126 })

// The board's page: the tickets so far, a form to add one, and new ones as they arrive.
const page = js(function html(board) {
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const items = Object.values(board).reverse().map((t) => `<li>${esc(t.title)}</li>`);
  return `<!doctype html>
<meta charset="utf-8">
<title>Live board</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}input{width:70%}li.new{animation:in 1s}@keyframes in{from{background:#ffd}}</style>
<h1>Live board</h1>
<form id="add"><input name="title" placeholder="A new ticket" required maxlength="80"> <button>Add</button></form>
<ul id="tickets">${items.join("\n")}</ul>
<script>
const list = document.getElementById("tickets");
new EventSource("/live").onmessage = (e) => {
  const li = document.createElement("li");
  li.className = "new";
  li.textContent = JSON.parse(e.data).title;
  list.prepend(li);
};
document.getElementById("add").onsubmit = (e) => {
  e.preventDefault();
  const title = e.target.elements.title.value;
  e.target.reset();
  fetch("/tickets", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ title }) });
};
</script>`;
})

// The board, with every ticket stored.
route.get("/", () =>
  respond(page(tickets.list()), { headers: { "content-type": "text/html; charset=utf-8" } }),
)

// Stores the ticket under a new key and tells every open board about it.
route.post("/tickets", (req) => {
  const title = req.json.title
  tickets[`${now}-${slice(sha256(as_text(random)), 0, 6)}`] = { title, at: now }
  updates.send({ title, at: now })
  return status(201, { ok: true, title }) ?? status(400)
})

// New tickets as server-sent events, for as long as the caller stays.
route.get("/live", () => updates.subscribe())
