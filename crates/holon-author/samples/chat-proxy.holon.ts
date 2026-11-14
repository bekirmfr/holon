// Chat proxy: a streamed reply.
//
// Asks Claude (the Messages API, streamed) what is posted, and forwards the answer to the caller
// as it arrives, so the page shows the words as they come. The flow still gets the whole answer
// once it ends, and stores the prompt with what was said. A call the API refuses (a bad key, a
// limit) forwards nothing: the reply is the API's answer, with its status. POST /room sends the
// answer's pieces to the channel room instead, for whoever watches GET /room/live. An answer has
// the run's 25 seconds (run_budget_ms in Settings for longer ones).
//
// Before it works, set its secret in the flow's Settings:
//   anthropic_api_key   an Anthropic API key (sk-ant-...), from console.anthropic.com
//
// Try it: deploy it, open http://127.0.0.1:8127/ and ask something. Or
//   curl -N -H 'content-type: application/json' -d '{"prompt":"a haiku about rivers"}' \
//     http://127.0.0.1:8127/chat
// prints the events as they come. For the room, in one terminal
//   curl -N http://127.0.0.1:8127/room/live
// and in another
//   curl -s -H 'content-type: application/json' -d '{"prompt":"a haiku about rivers"}' \
//     http://127.0.0.1:8127/room
// The first prints the pieces as they come, then the prompt with what was said.
import { as_text, flow, js, respond, route, status } from "holon"
import anthropic_api_key from "secret:anthropic_api_key"
import chats from "collection:chats"
import room from "channel:room"
import anthropic from "host:api.anthropic.com"
import { now } from "holon:clock"

flow("chat-proxy", { port: 8127 })

// The page: a box to ask in, the answer as it streams, and the last 10 exchanges.
const page = js(function html(past) {
  const esc = (s) =>
    String(s).replace(
      /[&<>"']/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
    )
  const items = Object.values(past)
    .reverse()
    .slice(0, 10)
    .map((c) => `<li><b>${esc(c.prompt)}</b><br>${esc(c.said)}</li>`)
  return `<!doctype html>
<meta charset="utf-8">
<title>Chat proxy</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}input{width:70%}#said{white-space:pre-wrap;min-height:3em;border-left:3px solid #ccd;padding-left:1em}</style>
<h1>Chat proxy</h1>
<form id="ask"><input name="prompt" placeholder="Ask something" required> <button>Ask</button></form>
<p id="said"></p>
<h2>Asked before</h2>
<ul id="past">${items.join("\n")}</ul>
<script>
const out = document.getElementById("said");
document.getElementById("ask").onsubmit = async (e) => {
  e.preventDefault();
  const prompt = e.target.elements.prompt.value;
  out.textContent = "";
  const r = await fetch("/chat", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ prompt }) });
  // Not an event stream: the model's API refused (a bad key, a limit), and says why.
  if (!(r.headers.get("content-type") || "").startsWith("text/event-stream")) {
    out.textContent = "the model's API answered " + r.status + ": " + (await r.text());
    return;
  }
  const reader = r.body.pipeThrough(new TextDecoderStream()).getReader();
  let rest = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    const lines = (rest + value).split("\\n");
    rest = lines.pop();
    for (const line of lines) {
      if (!line.startsWith("data:")) continue;
      const ev = JSON.parse(line.slice(5));
      if (ev.type === "content_block_delta" && ev.delta.type === "text_delta") out.textContent += ev.delta.text;
    }
  }
  // The exchange, once the answer has ended: on top of what was asked before, as the flow stored it.
  const li = document.createElement("li");
  const asked = document.createElement("b");
  asked.textContent = prompt;
  li.append(asked, document.createElement("br"), out.textContent);
  document.getElementById("past").prepend(li);
};
</script>`
})

// Reads the answer's text out of the stream's events.
const answer = js(function said(events) {
  let said = ""
  for (const line of String(events).split("\n")) {
    if (!line.startsWith("data:")) continue
    let e
    try {
      e = JSON.parse(line.slice(5))
    } catch (_) {
      continue
    }
    if (e.type === "content_block_delta" && e.delta && e.delta.type === "text_delta")
      said += e.delta.text
  }
  return said
})

// The page, with what was asked before.
route.get("/", () =>
  respond(page(chats.list()), { headers: { "content-type": "text/html; charset=utf-8" } }),
)

// Asks the model, streams its answer to the caller, and stores the prompt with what was said.
route.post("/chat", async (req) => {
  const prompt = req.json.prompt
  const sent = await anthropic.post("/v1/messages", {
    headers: {
      "content-type": "application/json",
      "x-api-key": anthropic_api_key,
      "anthropic-version": "2026-10-21",
      "anthropic-beta": "server-side-fallback-2026-07-01",
    },
    body: JSON.stringify({
      model: "claude-opus-5-5",
      max_tokens: 1024,
      stream: true,
      fallbacks: "default",
      output_config: { effort: "low" },
      messages: [{ role: "user", content: prompt }],
    }),
    stream: true,
  })
  chats[as_text(now)] = { prompt, said: answer(sent.body) }
  return respond(sent.body, { stream: true }) ?? status(400)
})

// Asks the model and pipes its answer to the room as it arrives, then sends the whole exchange.
route.post("/room", (req) => {
  const prompt = req.json.prompt
  const sent = room.pipe(
    anthropic.post("/v1/messages", {
      headers: {
        "content-type": "application/json",
        "x-api-key": anthropic_api_key,
        "anthropic-version": "2026-10-21",
        "anthropic-beta": "server-side-fallback-2026-07-01",
      },
      body: JSON.stringify({
        model: "claude-opus-5-5",
        max_tokens: 1024,
        stream: true,
        fallbacks: "default",
        output_config: { effort: "low" },
        messages: [{ role: "user", content: prompt }],
      }),
    }),
  )
  const said = answer(sent.body)
  room.send({ prompt, said })
  return { prompt, said } ?? status(400)
})

// What goes out on the room, as server-sent events.
route.get("/room/live", () => room.subscribe())
