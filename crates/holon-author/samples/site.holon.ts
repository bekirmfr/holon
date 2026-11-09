// Site: a web app and its API on one port.
//
// The routes a single-page app needs: its API under /api, and its page for every other path, so a
// deep link (/docs/setup) opens the app where it points, as history-mode routing expects.
// `/*path` is a rest segment: it matches the rest of the path, one segment or more, and
// req.params.path is what it matched (docs/setup), each segment decoded. A route without a rest
// beats any route with one, and among rest routes the longer prefix wins: /api/hello answers
// itself, any other /api path gets a JSON 404 from /api/*rest rather than the page, and every
// other path gets the page. A segment that is empty, . or .., or that decodes to a / or a \,
// matches no rest, so a rest only ever names what the request's path said. The page's routes are
// lazy: each request is recorded, on disk within about a second.
//
// Try it: deploy it, then open http://127.0.0.1:8196/docs/setup in a browser. Or with curl:
//   curl http://127.0.0.1:8196/api/hello answers {"hello":"world"};
//   curl http://127.0.0.1:8196/api/nope answers 404 with {"error":"no API at /api/nope"};
//   curl http://127.0.0.1:8196/docs/setup answers the page, opened at /docs/setup.
import { flow, js, respond, route, status } from "holon"

flow("site", { port: 8196 })

// The app's one page: where it was opened, and what its API says once the page has asked it.
const page = js(function html(path) {
  const esc = (s) =>
    String(s).replace(
      /[&<>"']/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
    )
  return `<!doctype html>
<meta charset="utf-8">
<title>Site</title>
<style>body{font:16px/1.5 system-ui;max-width:40em;margin:3em auto;padding:0 1em}code{background:#f3f3f3;padding:0 .3em}</style>
<h1>Site</h1>
<p>Opened at <code>/${esc(path)}</code>.</p>
<p id="api">Asking the API…</p>
<p><a href="/">Home</a> · <a href="/docs/setup">Setup</a> · <a href="/docs/a%20guide/part%202">A guide, part 2</a></p>
<script>
fetch("/api/hello").then((r) => r.json()).then((j) => {
  document.getElementById("api").textContent = "The API says hello, " + j.hello + "."
})
</script>`
})

// The API.
route.get("/api/hello", () => ({ hello: "world" }))

// Any other API path: a JSON 404, not the page.
route.get("/api/*rest", (req) => status(404, { error: `no API at /api/${req.params.rest}` }))

// The page at the root, which no rest matches.
route.get(
  "/",
  () => respond(page(""), { headers: { "content-type": "text/html; charset=utf-8" } }),
  { log: "lazy" },
)

// The page at every other path: a deep link opens the app where it points.
route.get(
  "/*path",
  (req) =>
    respond(page(req.params.path), { headers: { "content-type": "text/html; charset=utf-8" } }),
  { log: "lazy" },
)
