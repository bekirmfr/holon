// Drop board (samples/drop-board.holon) in TypeScript's syntax: the same unit.
import { flow, js, on, respond, route } from "holon"
import state from "state"
import board from "folder:board"
import drops from "channel:drops"

flow("drop-board", { port: 8157 })

const page = js(function html(count) {
  return `<!doctype html>
<meta charset="utf-8">
<title>Drop board</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}li.new{animation:in 1s}@keyframes in{from{background:#ffd}}</style>
<h1>Drop board</h1>
<p>${count} files dropped so far; each one dropped into the folder, or removed from it, shows below as it happens.</p>
<ul id="files"></ul>
<script>
const list = document.getElementById("files");
new EventSource("/live").onmessage = (e) => {
  const file = JSON.parse(e.data);
  const li = document.createElement("li");
  li.className = "new";
  li.textContent = file.event + " " + file.name + (file.event === "deleted" ? "" : " (" + file.size + " bytes)");
  list.prepend(li);
};
</script>`;
})

// A file dropped in: one more counted, and sent to whoever listens.
on.file.created(board, (f) => {
  state.files = (state.files ?? 0) + 1
  drops.send({ event: f.event, name: f.name, size: f.size })
})

on.file.deleted(board, (f) => {
  drops.send({ event: f.event, name: f.name, size: f.size })
})

route.get("/", () =>
  respond(page(state.files ?? 0), { headers: { "content-type": "text/html; charset=utf-8" } }),
)

route.get("/live", () => drops.subscribe())
