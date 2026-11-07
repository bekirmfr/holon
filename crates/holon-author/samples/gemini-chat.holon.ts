// Gemini chat: a streamed reply, on the free tier.
//
// Sends a prompt to Google's Gemini API (the Interactions API) and streams the answer to the
// caller as it arrives; the page shows the words as they come. Once the answer ends, the flow
// stores the prompt with what was said, read out of the events by a js holon. A call the API
// refuses (a bad key, the free tier's rate limit) forwards nothing: the reply is the API's answer,
// with its status, and the page says so. On the free tier Google may use what is sent to improve
// its products.
//
// Before it works, set its secret under the deployment's settings (the gear):
//   gemini_api_key   a Gemini API key, free from aistudio.google.com (Get API key)
//
// Try it: deploy it, open http://127.0.0.1:8128/ and ask something: the answer shows as it is
// written. Or
//   curl -N -H 'content-type: application/json' -d '{"prompt":"a haiku about rivers"}' \
//     http://127.0.0.1:8128/chat
// prints the events as they come. The page lists what was asked before.
import { as_text, flow, js, respond, route, status } from "holon"
import gemini_api_key from "secret:gemini_api_key"
import chats from "collection:chats"
import generativelanguage from "host:generativelanguage.googleapis.com"
import { now } from "holon:clock"

flow("gemini-chat", { port: 8128 })

// The page: a form whose answer streams in, and the last ten exchanges.
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
<title>Gemini chat</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}input{width:70%}#said{white-space:pre-wrap;min-height:3em;border-left:3px solid #ccd;padding-left:1em}</style>
<h1>Gemini chat</h1>
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
  // Not an event stream: the API refused (a bad key, the free tier's limit), and says why.
  if (!(r.headers.get("content-type") || "").startsWith("text/event-stream")) {
    out.textContent = "Gemini answered " + r.status + ": " + (await r.text());
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
      let ev;
      try { ev = JSON.parse(line.slice(5)); } catch (_) { continue; }
      if (ev.event_type === "step.delta" && ev.delta && ev.delta.type === "text") out.textContent += ev.delta.text;
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

// Reads the answer's text out of the stream's step.delta events.
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
    if (e.event_type === "step.delta" && e.delta && e.delta.type === "text") said += e.delta.text
  }
  return said
})

// Serves the page with what was asked before.
route.get("/", () =>
  respond(page(chats.list()), { headers: { "content-type": "text/html; charset=utf-8" } }),
)

// Streams Gemini's answer to the caller and keeps the prompt with what was said.
route.post("/chat", async (req) => {
  const prompt = req.json.prompt
  const sent = await generativelanguage.post("/v1beta/interactions", {
    headers: { "content-type": "application/json", "x-goog-api-key": gemini_api_key },
    body: JSON.stringify({ model: "gemini-3.5-flash-lite", input: prompt, stream: true }),
    stream: true,
  })
  chats[as_text(now)] = { prompt, said: answer(sent.body) }
  // Without a prompt no call is made, and the route answers 400.
  return respond(sent.body, { stream: true }) ?? status(400)
})
