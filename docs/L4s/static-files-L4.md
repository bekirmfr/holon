# Static files: a route for the rest of a path, and an answer that is a file's bytes (L4)

**Status:** design, before any code. Written 2026-11-04, its claims checked against the code at
`450e3de` and cited by file and line there. Builds on [named folders](named-folders-L4.md) (handles,
reads by hash) and [channels](channels-L4.md) (an answer the host streams after the run).
`FileServe` is a new item, so §8 answers [host capabilities](host-capabilities-L4.md) §4's
checklist. The rest segment is routing, not a capability.

Two features that together let one flow serve a built web app (a Vite or React `dist/`) and its
API:

- **A rest segment** (`*path`): a route that matches the rest of a path, however many segments.
- **Answering with a file** (`site.serve(name)`): a route answers with a file's bytes from a
  named folder, any size, any type (text or binary), streamed by the host and never through the
  flow.

```
flow site
port 8131
folder site

route GET /api/hello (req) {
  respond { hello: "world" }
}

route GET / (req) {
  respond site.serve("index.html"), missing: 404
}

route GET /*path (req) {
  respond site.serve(req.params.path) ?? site.serve("index.html"), missing: 404
}
```

`check` says:

```
a server on :8131
  GET /api/hello
  GET /
  GET /*path
it may:
  receive requests
  read files in the folder "site"
  answer its callers with files from the folder "site"
```

The bytes a caller gets are the bytes of the file whose hash the run recorded, or the caller gets
nothing.

---

## 1. What is already there

- **Routes** are parsed by `parse_path` (`holon-core/src/declare.rs` 305): literals and `:name`
  params, nothing else. `match_route` (332) matches only routes with the same number of segments
  as the request, and the most literal segments wins, ties to the earlier route. Two routes
  clash when their segments are the same with every param treated alike (240).
- **Three other copies of the path's shape** have to agree with core: the language reads a
  route's path as a run of tokens (`holon-author/src/lang/parse.rs` 577); the wasm bridge hands the
  editor the segments (`holon-wasm/src/lib.rs` 897, `route_path`); and the editor matches requests
  itself for its request tester (`editor/src/endpoint.ts` 16, 39–46, 115). The author's routes
  module has its own shape for clashes (`holon-author/src/routes.rs` 226).
- **A response** is read from the endpoint's output 0 (`holon/src/data.rs` 163): a value, an
  envelope `{status, headers, body}`, an effect (204), or a **subscription**: a
  `channel_subscribe` effect, recognized by `subscription` (51) and streamed by the transport. The
  flow can't set framing headers (`FRAMING`, 48). A subscription is the precedent for an answer
  the host delivers after the run, from a value the run computed.
- **Letting callers subscribe is its own permission,** `ChannelRead("updates")` (`kind.rs` 149,
  337), described as "let callers subscribe to its channel" (`holon-author/src/describe.rs` 47),
  apart from the flow sending on it. Answering with a secret is likewise its own,
  `SecretAnswer(name)` (`kind.rs` 169). Publishing something to callers is shown and approved on
  its own.
- **A handle** (`inbox.file(name)`, `Folders::handle`, `holon/src/folders.rs` 490) is
  `{folder, name, size, hash, modified}`: the file hashed whole, its bytes kept nowhere. The run
  log keeps the handle as a read.
- **A file sent by handle** is `http_upload` (`primitives.rs` 877): the host opens the file,
  checks it against the handle's hash before a byte is sent, and again as it sends
  (`folders.rs` 1441–1460, `Sent`; `outbound.rs` 46, `FileBody`). A file that changed fails the
  call with `changed`. The log keeps the handle, never the bytes.
- **The data plane** is hyper (http1) on tokio (`holon/src/dataplane.rs`), whose `Answer` (146) is
  a whole body, a forwarded stream or an event stream. A file is a fourth kind.
- **The log** keeps a response's body as text cut at 64 KiB (`recorder.rs` 93), and every
  request, routed or not (`listeners.rs` 208).

## 2. The rest segment

**Syntax.** `*name` as the last segment of a route's path: `/app/*path`, `/*path`. One per path,
only last; anything else is a bad path, as a repeated param name is today.

**What it matches.** One or more segments, the rest of the path. Never zero: `/app/*path` doesn't
match `/app` or `/app/`, as `:id` never matches an empty segment (`declare.rs` 352). A flow that
wants `/` too declares `GET /` beside `GET /*path`, as the example does.

**Its value.** `req.params.path` is the rest, its segments percent-decoded one by one and joined
with `/`: `/app/css/site%20a.css` gives `css/site a.css`. A segment that decodes to contain `/`,
or that is `.` or `..`, makes the route not match (404). Folders refuse such names anyway
(`folders.rs`, `bad_file_name`), but the parameter should never carry a path that means something
other than what the request said.

**Which route wins.** A route without a rest segment beats any route with one, whatever their
literals. Among rest routes, the most literal segments wins (the longest prefix). Ties go to the
earlier route, as now. So `GET /api/hello` and `GET /api/:name` both beat `GET /*path`, and
`GET /docs/*page` beats `GET /*path` for `/docs/a/b`.

**Clashes.** Two rest routes clash when their segments before the rest are the same with params
treated alike. A rest route never clashes with a route without one.

**Status for other methods.** A 405's `allow` header lists the methods with a matching route,
rest routes included (`data.rs` 104–113, unchanged).

**What it changes about what a flow may do.** Nothing: the route is listed like any other
(`GET /*path`), the request is logged as now, and its param is text from the request. One thing
does change in practice: a flow with `GET /*path` runs a handler for every GET, so requests that
would have been 404s without a run now run and are logged. That is the flow's choice, shown in
its route list.

**Where it is built.** `Segment::Rest(name)` in core (`parse_path`, `match_route`, the clash
check); the token in the language's route path (`Tok::Star`, `parse.rs` 577) and the printer; the
wasm bridge's `route_path` (`rest: Some(name)`); the editor's `parsePath` and matcher, kept equal
to core's by a test over the same table of paths and requests; `routes.rs`'s shape. Docs:
`LANGUAGE.md` (routes), the manual's reference.

## 3. Answering with a file

### The language and the node

`site.serve(name)` lowers to `file_serve{folder: site}(file_handle{folder: site}(name))`:

- **`file_handle`** is today's leaf. It hashes the file and is logged as a read; a missing file is
  absent, so `missing: 404` and `??` work as for any read.
- **`file_serve`** is a new primitive. Pure, like `channel_subscribe` (`catalog.rs` 353), it
  turns a handle into an effect value, `Effect { sink: "file_serve:site", payload: {folder,
  name, size, hash, type} }`. `type` is the content type core reads from the name's extension
  (a fixed table in core, so it is part of the run and replays). It's absent when the handle is
  absent or names another folder.
- **Its capability** is `FileServe("site")`, a sink like `ChannelRead`, described as "answer its
  callers with files from the folder \"site\"". `FileRead("site")` still comes from the handle
  leaf. A deploy asks for both; a host policy can refuse either.

`respond site.serve(n)` answers with the file. So does an envelope whose body is one, and the
envelope's headers are added: `respond site.serve(n), headers: { "cache-control": "max-age=31536000, immutable" }`.

The wiring form can feed `file_serve` any handle, for example a handle a flow computed from a
listing. The host checks that the handle's folder is the node's folder and one the deployment
reads, as `Sender` checks an upload's (`folders.rs` 1385).

### What the host does

`respond` (`data.rs` 163) recognizes the effect as it recognizes a subscription, and returns a
`DataResponse` with a new field, `file: Some(Served {folder, name, size, hash})`. After the run
has committed, the transport (`dataplane.rs`) answers:

1. **Headers.** `content-type` from `type` unless the flow set one; `content-length` the
   handle's size; `etag: "<hash>"`; `cache-control: no-cache` unless the flow set one
   (revalidate each time, which with the ETag costs a 304).
2. **Conditional requests.** `If-None-Match` that names the hash: 304, no body. The run still
   ran: which file a path serves is the flow's to say, every time.
3. **The bytes.** Opened through the deployment's `Scoped` folders (confined as every read is),
   checked against the handle:
   - **Small files** (up to `inline_file_bytes`, 1 MiB by default) are read once, hashed, and
     sent from memory if the hash matches. The bytes hashed are the bytes sent.
   - **Bigger files** stream, hashed as they go, after a check of size and modified time against
     the handle. If the hash doesn't match at the end, or the size isn't the handle's, the
     connection is cut before the last chunk. The caller's client sees a short body against the
     `content-length` and drops it.
   - A file that changed before a byte went out answers **503** with `retry-after: 1`. The log
     says `changed`, as an upload's does.
4. **HEAD and ranges** are a later slice (§8).

### The handle's cost

A handle hashes the whole file on every request: about a millisecond per few MB with BLAKE3. For
a bundle of a few hundred KB that doesn't matter; for video it does. The host may keep a handle
cache by `(folder, name, size, modified, inode)`. It is safe because the send always checks the
bytes against the hash: a cache that lies (a file rewritten within the same second, at the same
size) gets a 503 or a cut connection, never wrong bytes under the right hash. It is an
optimization, and it isn't in the first slice.

### Files, and bytes a run computed

This is not a binary value in core. Host capabilities plans one, `Bytes` (§5.10, step 1.1, fork
21), and the two fit together:

- **`serve` is for files.** A file of any size never enters a run whole (named-folders §4). The
  run holds a handle, and the host sends the bytes. That stays true after `Bytes` lands: a 40 MB
  video is never a value.
- **`Bytes` is for what a run computes**: a generated PNG, a zip, a signed PDF, within the run's
  limits. Once value-kinds lands, `respond b` with a `Bytes` value sends the bytes as they are
  (§5.10: "an HTTP body that is bytes is sent as they are"), with `application/octet-stream`
  unless the flow sets a type. It needs nothing from this note, and this note needs nothing from
  it.

### Folder backends

A folder bound to SFTP or S3 (host capabilities §6) serves the same way: the host streams from
the backend and checks the hash as it sends. Redirecting the caller to a presigned S3 URL would be
faster, but the host would never see the bytes, so it couldn't stand behind the hash. Refused
(§9).

## 4. The log, replay and verify

- **The run** is logged as any request's: the request, the handle read (`{name, size, hash,
  modified}`, tens of bytes), and output 0, the `file_serve` effect.
- **`response_body`** is `file site/assets/app.3f9a1c.js (148.2 KB, blake3 7c1e…)` instead of the
  bytes; `status` is what was sent (200, 304, 503).
- **Replay and verify** re-run from the logged handle and compare output 0. The bytes the caller
  got are proven by the hash in the chain. Whether they can be produced again depends on the
  folder: a folder keeps reads `bytes` or `hash`, and a handle is always kept by hash
  (named-folders-L4 §6). A file served a thousand times costs a thousand small lines, never a
  thousand copies.
- **A cut connection** (§3, step 3) is logged with `error: "changed while sent"`.

## 5. Limits

- **Size:** a deployment's `max_served_bytes` per file (default 1 GiB, capped by the host policy),
  past which `file_serve` answers 500 and the log says why. Nothing about the size enters a run.
- **Connections:** a streaming answer holds a connection, so it counts toward `max_connections`
  (`dataplane.rs` 48) as a subscription does.
- **A run per request.** Every file request runs the flow and writes a log line. This is the
  price of "every request is recorded". A deployment serving many assets should sit behind a
  CDN, and then Holon records what reaches it, not every visitor.

## 6. Host capabilities' checklist (§4)

1. **The item and its words:** `FileServe("site")`, from `site.serve(…)` (or a `file_serve`
   node): "answer its callers with files from the folder \"site\"". Per folder, the grain
   `FileRead` already has.
2. **The resource:** the folder an admin binds, as for `FileRead`. A deploy is refused until it
   is bound (named-folders §2). No credentials.
3. **Reads are the run's input:** the handle is a logged read (§4). The bytes sent are not an
   input; they are checked against it.
4. **Effects land when the run finishes, at most once:** the answer goes out after the run
   commits. A run that runs again (claims) answers once, from its last attempt. A file that
   changed is a 503, never different bytes.
5. **Sharing:** nothing new. A run that writes the file it serves serves the handle it read, so
   the write isn't visible to this caller, and the send then fails the hash check (503) if the
   write landed first.
6. **Triggers:** none; it answers a route.
7. **Secrets:** a file isn't a secret, and `taint.rs` sees no secret in a handle. A flow that
   writes a secret-derived text to a folder is already refused (named-folders), so serving can't
   carry one out.
8. **Limits:** `max_served_bytes` (§5), with a default and a ceiling in the policy, shown in the
   deploy bar.
9. **Verify:** recomputes the handle and the answer; the bytes are checked by hash (§4).
10. **Seeing it:** `holon check`'s words, the deploy bar, the manual's permissions page and its
    files guide, and the run log's line (§4).
11. **The sample:** `site`, deployed by the e2e tests against a fixture folder (§7). No outside
    service is involved.
12. **The engine:** none new. hyper's streaming body, as subscriptions use.

## 7. Samples

- **`site`** (the example above): a folder bound to a built app, its API beside it, and a
  history-mode fallback to `index.html`. The e2e test binds the folder to a fixture `dist/` (an
  `index.html`, a script, a PNG) and checks a deep link gets the page, the script its type, the PNG
  its exact bytes, a second request a 304, and a missing asset 404 under `/assets/`.
- `log-archive` gains `GET /logs/:name/download` answering with the whole log: the binary-safe,
  any-size counterpart of its `tail`.

## 8. Slices

1. **The rest segment:** core, language, printer, bridge, editor matcher, docs; the `site` sample
   without `serve` (an index page from a `js` holon, a catch-all answering it).
2. **`file_serve`:** the primitive, the capability and its description, `respond`, the
   transport's file answer, small files from memory, big ones streamed and checked, ETag and 304,
   the log line; the `site` sample in full, `log-archive`'s download.
3. **Later:** `HEAD`; `Range` (206, for video and resumable downloads, checked against the whole
   file's hash with the BLAKE3 tree proofs ranges.rs already makes); precompressed siblings
   (`app.js.br` served for `app.js` when the caller accepts it, each file checked by its own
   hash); the handle cache.

## 9. Refused, with the reason

| Refused | Why |
|---|---|
| A static mount by host path (`static "/var/www"`) | The flow would see a path, the version wouldn't cover what it serves, and files could change under a fixed hash with nothing recorded. Folders are bound by an admin and read by hash. |
| Recognizing a plain handle record in `respond` as a file | A flow answering `respond inbox.file(n)` today sends the handle as JSON. Changing that would change existing flows, and a record isn't a permission. `serve` is explicit and asks `FileServe`. |
| A redirect to a backend's presigned URL | The host would never see the bytes, so nothing stands behind the hash in the log. |
| Serving without a run | It would make a request the log can't account for. A run per request is the recorded path; a CDN in front is the fast one. |

## 10. Forks, surfaced

- **Rest matching zero segments** (`/*path` also matching `/`) would save the second route in
  the sample. Refused for now for consistency with `:param`, which is never empty. Revisit if
  every sample needs both.
- **A folder bound as `--no-serve`**, so an admin can keep a folder from ever being answered with
  whatever the flow asks. The deploy's approval already shows `FileServe`; add this if a team
  asks.
- **Content types:** core's table covers the web's types (html, css, js, mjs, json, map, svg,
  png, jpg, gif, webp, avif, ico, woff, woff2, ttf, wasm, txt, pdf, mp4, webm, mp3). Anything
  else is `application/octet-stream` unless the flow sets one.
- **Assets bundled with the version** (a flow's own files, content-addressed into its unit, so
  they are covered by its hash and need no folder) are the natural next step. They are not
  designed here.
- **Where it sits in host capabilities' plan (§7).** The plan schedules capabilities, not this
  note (R4), and the plan doesn't list this one yet. It needs nothing from Tier 1, `Bytes`
  included, and the rest segment needs nothing at all, so it can go anywhere. *Lean:* a step of
  its own in Tier 1, beside 1.7, since a flow has no way today to answer with a file it doesn't
  read whole.

## 11. Invariant check

- **I2 purity:** `file_serve` is pure: a handle in, an effect value out, the type from a table
  in core. The host reads the bytes after the run, as it streams a subscription. ✓
- **I5 content addressing:** the rest segment is part of the endpoint's `path` param, so of its
  hash; no unit that exists today changes. ✓
- **I7 the effect boundary:** the answer is an `Effect` the host performs, never a value a node
  reads. ✓
- **I8 derived labels:** `FileServe(folder)` is read from the node's param, not declared; the
  route list shows the rest segment. ✓
- **The record:** every request is still a run and a log line; what was sent is recorded by
  hash; a caller never gets bytes other than the recorded hash's. ✓
