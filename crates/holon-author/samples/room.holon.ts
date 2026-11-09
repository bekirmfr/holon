// Room: a chat over WebSockets, each of a connection's blocks a run.
//
// A page opens a WebSocket on /room/<name>. Its opening runs open: the flow keeps who joined, by
// the connection's id, tells the room, and subscribes the connection to the channel room, so each
// pack sent on it reaches every page in the room. Each text a page sends runs message: it is said
// to the room under its sender's name. When a page goes, close runs once: the flow forgets them
// and tells the room. A page from another site can't open one, and each connection may send ten
// messages a second (limits, under ⚙).
//
// Try it: deploy it, open http://127.0.0.1:8132/ in two browsers, join under two names and say
// something in one: it shows in both; close one and the other says who left.
import { flow, respond, route } from "holon"
import people from "collection:people"
import room from "channel:room"

flow("room", { port: 8132 })

// The room's page: a name to join under, a line to say something, and what is said.
const page = `<!doctype html>
<meta charset="utf-8">
<title>Room</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}input{width:70%}ul{list-style:none;padding:0}.note{color:#888}</style>
<h1>Room</h1>
<form id="join"><input name="name" placeholder="Your name" required maxlength="24" pattern="[A-Za-z0-9_]+" title="letters, digits and _"> <button>Join</button></form>
<form id="say" hidden><input name="text" placeholder="Say something" required maxlength="200" autocomplete="off"> <button>Say</button></form>
<ul id="said"></ul>
<script>
const said = document.getElementById("said");
const line = (text, note) => {
  const li = document.createElement("li");
  li.textContent = text;
  if (note) li.className = "note";
  said.append(li);
};
let ws;
document.getElementById("join").onsubmit = (e) => {
  e.preventDefault();
  const name = e.target.elements.name.value;
  ws = new WebSocket("ws://" + location.host + "/room/" + encodeURIComponent(name));
  ws.onopen = () => line("you joined as " + name, true);
  ws.onmessage = (m) => {
    const p = JSON.parse(m.data);
    if (p.joined) line(p.joined + " joined", true);
    else if (p.left) line(p.left + " left", true);
    else line(p.from + ": " + p.text);
  };
  ws.onclose = (c) => line("closed (" + c.code + (c.reason ? ": " + c.reason : "") + ")", true);
  e.target.hidden = true;
  document.getElementById("say").hidden = false;
};
document.getElementById("say").onsubmit = (e) => {
  e.preventDefault();
  ws.send(e.target.elements.text.value);
  e.target.reset();
};
</script>`

route.get("/", () => respond(page, { headers: { "content-type": "text/html; charset=utf-8" } }))

// A connection on /room/<name>: open keeps who joined and subscribes them to the room, message
// says the text to it under their name, close forgets them and tells it they left.
route.websocket("/room/:name", {
  open: (req) => {
    people[req.conn] = req.params.name
    room.send({ joined: req.params.name })
    return room.subscribe()
  },
  message: (m) => {
    room.send({ from: people[m.conn], text: m.text })
  },
  close: (c) => {
    delete people[c.conn]
    room.send({ left: c.params.name })
  },
})
