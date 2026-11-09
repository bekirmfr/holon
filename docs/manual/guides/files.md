# Files on the host

A flow can read and write files on the server's machine, but only in folders an admin has bound
to a name, and only by that name. The flow says `import inbox from "folder:inbox"`; the admin
decides that `inbox` is `/srv/drop`, read only. The flow never sees a path, and can't reach
anything outside the folders it names.

## Binding a folder

An admin binds folders from the editor's **admin** page (**folders**), or on the server's
machine. On the admin page, **browse…** next to the path chooses a folder on the server's
machine: when the editor is open on that machine, the system's own folder dialog opens there;
from anywhere else, or with **browse here instead**, the editor lists the server's folders to
click through. `HOLON_NO_FOLDER_DIALOG=1` keeps the system's dialog from opening. On the command
line:

```sh
holon folder add inbox /srv/drop --read-only
holon folder add reports /srv/reports
holon folder list
holon folder remove reports
```

| Option | |
|---|---|
| `--read-only` | flows may read it, not write it |
| `--for SLUG` | for that deployment only: it sees this folder as the name, and no other deployment sees it |
| `--keep hash` | the run log keeps a hash of what each run read, not the text |
| `--keep 30d` | the log keeps the text for 30 days (`s`, `m`, `h` or `d`), then only its hash |
| `--cap 10M` | a write that would take the folder's files past 10 MiB in all doesn't land |
| `--shared` | other servers bind this folder too (a network share): their runs' writes exclude each other as one server's do |
| `--lock run` | each run that touches it holds it for its whole length: shared to read, alone to write (below) |

- A name is letters, digits, `-` and `_`, up to 64 characters.
- `--for` lets two deployments use the same name for different folders: bind `inbox` for `acme`
  to `/srv/acme`, and for `globex` to `/srv/globex`.
- The server's own data folder can't be bound.
- Removing a binding leaves the files where they are; deployments that use it stop until it is
  bound again.

A deploy is refused until each folder the flow names is bound, and bound read-write where the
flow writes.

## Reading and writing

A folder reads and writes like a collection, by file name:

```ts
import { flow, length, route, status } from "holon"
import inbox from "folder:inbox"
import reports from "folder:reports"

flow("inbox-report", { port: 8112 })

route.post("/report", (req) => {
  const name = req.json.name
  const text = inbox[name]
  reports[`${name}.md`] = `# ${name}\n\n${length(text.split("\n"))} lines`
  return { report: `${name}.md` }
})

route.get("/reports", () => reports.list())

route.delete("/reports/:name", (req) => {
  delete reports[req.params.name]
  return status(204)
})
```

| Written | Does | Asks for |
|---|---|---|
| `inbox[name]` | the file's text, as the run began; no value if there is none | read files in the folder "inbox" |
| `reports[name] = text` | write the whole file when the run finishes | write, copy into and delete files in the folder "reports" |
| `delete reports[name]` | delete it when the run finishes | the same |
| `reports.list({ prefix: p, after: a, limit: n })` | a page of files: name to `{size, hash, modified}`, in name order | read files |

- A name is a path inside the folder (`2026/10/a.csv`), and never leaves it.
- A file read whole is text (UTF-8), up to 1 MiB; a run reads 64 MiB in all at most. Reading
  something that isn't text fails the run, and it writes nothing.
- Writes land together when the run finishes, with the run's state and collection writes, or not
  at all. A write that would take a capped folder past its cap doesn't land, and the run answers
  500.
- Several deployments can read and write one folder. A run commits only if every file it read is
  as it found it, and the check and the writes hold a lock on each file it read or writes (and on
  the folder, for one it listed): a run of another deployment that wrote a file in between makes
  it run again, never write over what that one wrote, and runs that touch different files never
  wait for each other. The `shared-ledger` sample, deployed twice, shows it.
- A run that reads a file of a folder it may write, and then calls out, holds the file from its
  first outbound call until it commits (a listing, the whole folder): another run that read it
  waits at its own first call, reads it again, and runs on what it finds, having sent nothing.
  So two runs that fetch something to cache in one file fetch it once, across deployments.
- A folder bound `--lock run` is held by each run that touches it from before it starts until it
  commits, fails or waits: shared by runs whose flow only reads it, alone by one that may write
  it. Every run that writes it then goes one at a time, for its whole length, across every
  deployment bound to it (and, bound `--shared` too, every server: the lock is also an OS lock on
  `.holon-tmp-run.lock` in it). For files a program outside Holon expects one writer for. A run
  that would wait past `claim_wait_ms` is turned away (503), having done nothing.

## Big files

A file of any size is used without it entering the run whole:

```ts
import { flow, route } from "holon"
import archive from "folder:archive"
import logs from "folder:logs"

flow("log-archive", { port: 8124 })

route.get("/logs/:name/tail", (req) => logs.lines(req.params.name, { from: -20, limit: 20 }))

route.get("/logs/:name/head", (req) => logs.slice(req.params.name, { offset: 0, length: 256 }))

route.post("/archive", (req) => {
  const log = logs.file(req.json.name)
  archive[`${log.name}.${log.hash}`] = log
  return { archived: `${log.name}.${log.hash}`, size: log.size }
})
```

| Written | Gives |
|---|---|
| `logs.file(name)` | a **handle**: `{folder, name, size, hash, modified}`, without the bytes |
| `logs.lines(name, { from: i, limit: n })` | `n` lines from line `i` (from the end when negative), without line endings |
| `logs.slice(name, { offset: o, length: n })` | `n` bytes from `o` (from the end when negative), as text |
| `archive[n] = handle`, `archive.copy(n, handle)` | copy the file when the run finishes, on the host; it fails if the file no longer has the handle's hash |

Only the lines or bytes read enter the run, and the log keeps with them the hashes that prove
them part of the file, so the run can still be checked offline.

## Serving a folder

A route answers with a file of a folder with `f.serve(name)`: a built web app, a download. The
`site` sample serves a single-page app's build, its API beside it:

```ts
import { flow, respond, route, status } from "holon"
import site from "folder:site"

flow("site", { port: 8196 })

route.get("/api/hello", () => ({ hello: "world" }))

route.get("/api/*rest", (req) => status(404, { error: `no API at /api/${req.params.rest}` }))

route.get(
  "/assets/*file",
  (req) =>
    respond(site.serve(`assets/${req.params.file}`), {
      headers: { "cache-control": "public, max-age=31536000, immutable" },
    }),
  { log: "lazy" },
)

route.get("/", () => site.serve("index.html"), { log: "lazy" })

route.get("/*path", (req) => site.serve(req.params.path) ?? site.serve("index.html"), {
  log: "lazy",
})
```

`site.serve(name)` is the file's handle, served: the run reads its handle (its size and hash),
never its bytes, and the host sends them once the run is over.

- **Its type** is its name's: `text/html; charset=utf-8`, `text/javascript`, `text/css`,
  `image/png`, `font/woff2`, … (html, css, js, mjs, json, map, svg, png, jpg, gif, webp, avif,
  ico, woff, woff2, ttf, wasm, txt, pdf, mp4, webm, mp3), else `application/octet-stream`, unless
  the route's headers set one.
- **Its `etag`** is its hash, and `cache-control` is `no-cache` unless the route sets one: a
  browser asks again each time, and a copy that is still the file's gets a `304` with no body. A
  status the flow chose stays (`status(404, site.serve("404.html"))`); only a `200` becomes a
  `304`.
- **It is the file the run saw, or nothing.** A file up to `inline_file_bytes` (1 MiB) is read,
  and sent if its hash is the handle's; a bigger one is checked by its size and time, then
  streamed, its hash checked as it goes, and if it changes while it is sent the connection is cut
  before its end, and the deployment's timeline says so. A file changed or gone since the run read
  its handle is a `503` the browser may retry.
- **No such file is no value,** so `??` falls back: `site.serve(path) ?? site.serve("index.html")`
  is the file at the path, or the app's page, which is how a deep link opens a single-page app
  where it points. A name no file can have (`/Talk:Page`, a folder's own name) is no such file.
  A route answering with no value answers 404, as the asset route does for a script that isn't
  there: never the page served as a script.
- **It asks for `FileServe("site")`** beside `FileRead("site")`: "answer its callers with files
  from the folder site". A file past `max_served_bytes` (1 GiB) isn't sent: the caller gets a
  500 saying so.
- **The run log keeps the file by its hash,** `file site/assets/app.js (148.2 KiB, blake3
  7c1e2a9b…)`, never its bytes, and `holon verify` re-runs it from the handle the log kept.

The editor's request tester shows a small file's bytes; a big one is only streamed on the
deployment's own port, and the tester shows its line.

## A run per new file

`on.file.created(NAME, (f) => { … })` runs once for each new file in the folder `NAME` imports,
with the file's handle as `f` (its `event` says `new`, `changed` or `deleted`):

```ts
import { flow, on } from "holon"
import receipts from "folder:receipts"
import drop from "folder:drop"
import httpbin from "host:httpbin.org"

flow("drop-box")

on.file.created(drop, async (f) => {
  const sent = await httpbin.put("/put", {
    headers: { "content-type": "application/octet-stream" },
    file: f,
  })
  receipts[`${f.name}.json`] = JSON.stringify({ file: f.name, size: f.size, status: sent.status })
})
```

It asks to "run on each file new, changed or removed in the folder "drop" (as its on file lines say), and pass it on": that is its trigger.

- A file is new once it stops changing: the server looks at the folder every second, and a file
  with the same size and time at two looks in a row starts a run.
- Files already in the folder when the flow is first deployed aren't new.
- Files that arrive while the server is down, or the deployment paused, start their runs when it
  is back.
- A file the flow's own runs wrote into the folder doesn't start it again, unless it changes.
- A run on a file has the clock run's time budget.

### Beside routes

A file handler sits beside routes, a clock or actions, as any handler does:

```ts
import { flow, on, route } from "holon"
import files from "channel:files"
import drop from "folder:drop"

flow("drop-board", { port: 8206 })

on.file.created(drop, (f) => {
  files.send({ name: f.name, size: f.size })
})

route.get("/live", () => files.subscribe())
```

The handler is everything tied to the file's arrival
([What a handler holds](../language/reference.md#what-a-handler-holds)).

### Several handlers, some names

A flow may have several file handlers, each a trigger of its own. `on.file.deleted` runs for files
removed from the folder, and `{ matching: "*.csv" }` for the names that match (`*` and `?` within
a folder, `**/` any number of folders, letters either case):

```
on.file.created(inbox, (f) => { … }, { matching: "*.csv" })
on.file.deleted(inbox, (f) => { … })
on.file.created(outbox, (f) => { … })
```

A removed file's handle is as it was last seen, without a hash: its bytes are gone.

## Sending a file

`file` in place of `body` sends a file as a request's body, by handle:

```
await files.put(`/upload/${log.name}`, { file: log })
```

The server streams the bytes from the folder; they never pass through the flow, so a file of any
size can be sent. Before and as it sends, it checks they still have the handle's hash; if the file
changed, the call answers `{status: 0, error: "changed"}`. The run log keeps the request with the
handle, never the bytes. The file must be in a folder the flow reads, or be the new file that
started the run. Any method works: `await files.post(path, { file: h })` too.

## What the run log keeps

Replaying a run needs what it read, so the log keeps it:

- a text up to 4 KiB, in the run's entry;
- a bigger one beside the log, by its hash, in chunks: a file that grows adds only its new
  chunks, not a new copy;
- for a folder bound `--keep hash`, only the hash. `holon verify --files DIR` then checks those
  runs against the original files;
- for `--keep 30d`, the text for 30 days, then only the hash.

What a run wrote is kept by its hash. The server sweeps, every ten minutes, the texts no kept run
refers to; `holon folder sweep` does it at once.

`holon export` takes `--max-files-bytes 10M` to ship at most that much of those texts with the
runs (the oldest runs' first), or `--without-files` for none; it says what it left out. See
[Checking runs](checking-runs.md).
