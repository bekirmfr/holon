// Site: a built web app and its API on one port.
//
// Serves a single-page app built into a folder (a Vite or React build's dist/) with its API
// beside it. site.serve(name) answers with a file of the folder: the host reads it again as it
// sends it, and sends it only if it is still the file the run saw (else a 503 the browser may
// retry), with the type its name says and its hash as its etag, so a browser's cached copy that is
// still the file's gets a 304. A big file is streamed, never held whole. Every path that isn't a
// file gets the app's page, so a deep link (/docs/setup) opens the app where it points, as
// history-mode routing expects; a missing asset is a 404, never the page served as a script.
// /*path is a rest segment: it matches the rest of the path, one segment or more, and
// req.params.path is what it matched, decoded. A route without a rest beats any route with one,
// and among rest routes the longer prefix wins: /api/hello answers itself, any other /api path
// gets a JSON 404, /assets/… the build's assets, and every other path a file or the page. The
// page's routes are lazy: each request is recorded, on disk within about a second.
//
// Before it works, an admin binds the folder to the app's build:
//   holon folder add site ~/my-app/dist --read-only
//
// Try it: deploy it, then open http://127.0.0.1:8196/docs/setup in a browser. Or with curl:
//   curl http://127.0.0.1:8196/api/hello answers {"hello":"world"};
//   curl -i http://127.0.0.1:8196/docs/setup answers index.html, its etag the file's hash;
//   curl -i http://127.0.0.1:8196/assets/nope.js answers 404.
import { flow, respond, route, status } from "holon"
import site from "folder:site"

flow("site", { port: 8196 })

// The API.
route.get("/api/hello", () => ({ hello: "world" }))

// Any other API path: a JSON 404, not the page.
route.get("/api/*rest", (req) => status(404, { error: `no API at /api/${req.params.rest}` }))

// The build's assets, their names carrying their hashes, so a browser keeps them a year; one that
// isn't there is a 404.
route.get(
  "/assets/*file",
  (req) =>
    respond(site.serve(`assets/${req.params.file}`), {
      headers: { "cache-control": "public, max-age=31536000, immutable" },
    }),
  { log: "lazy" },
)

// The page at the root, which no rest matches.
route.get("/", () => site.serve("index.html"), { log: "lazy" })

// A file of the build at its path (favicon.ico, robots.txt), else the page: a deep link opens the
// app where it points.
route.get("/*path", (req) => site.serve(req.params.path) ?? site.serve("index.html"), {
  log: "lazy",
})
