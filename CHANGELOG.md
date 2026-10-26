# Changelog

## 0.1.0-rc.25

Nothing to redeploy: no unit is hashed differently, and deployments, logs and waiting runs from
rc.24 load as they are (a waiting run is sealed the next time the deployments file is written).
This release fixes what a security review of the server found.

- **An agent's changed code doesn't inherit a secret.** A grant approved what the code did with
  a secret, so when an agent deploys a new version of a flow that reads one, a person approves it
  again: a required secret saves the version for approval, an optional one deploys withheld (its
  reads refused, shown as revoked) until a person grants it back from the deployment's
  permissions. An agent no longer starts a paused or stopped deployment, and a redeploy keeps the
  limits a person lowered.
- **What a run computes from a secret stays out of the log.** The log redacted a secret by its
  value, so base64 of it (a Basic auth header), a field of a JSON secret or a slice reached the
  log, the export and the editor's replay as is. The log now keeps no outputs for a node that
  computes with a secret, and an outbound request built from one reads `‹withheld: computed from
  a secret›` there. One-way leaves (`sha256`, `hmac_sha256`, `secure_equals`) still log, so signing
  flows replay and verify as before; `holon verify` counts the other runs as not run again.
- **A clock run's history is redacted** like its log entry, and the server no longer prints a
  tick's effects.
- **A resume token resumes its run once**, however many requests carry it at the same time; the
  others answer 404 while it runs.
- **A waiting run's record is sealed on disk**, under the secrets master key, and the deployments
  file is written owner-only.
- **Outbound requests: the host sets `Host` and the framing headers.** A flow setting `Host`,
  `Content-Length`, `Transfer-Encoding`, `Connection`, `Keep-Alive`, `TE`, `Trailer`, `Upgrade`,
  `Expect` or a `Proxy-*` header gets `bad_header`, and nothing is sent. NAT64 addresses
  (64:ff9b::/96) count as private.
- **Accounts:** a password reset stops the old password at once; setting a new password ends
  the account's other sessions (a change keeps the one making it).
- **Revoking what starts a deployment's runs pauses it**, even with "keep running when something
  required is revoked" ticked.
- **Folders:** none may be or hold the `--dotenv` file, the secrets key's file, or the flow
  logs, arrivals and heads the server keeps beside its deployments file.
- **Bounded units:** a deploy whose unit holds more than 20,000 units or nests deeper than 64 is
  refused (400); working out a unit's permissions visits each unit once. A module that claims an
  answer past its memory fails as bad output instead of making the server allocate it.

## 0.1.0-rc.24

Flows with a streamed reply (`respond sent.body, stream: true`, the `chat-proxy` sample) are
hashed differently: redeploy them to get the change below. Every other flow is the unit it was,
and keeps running.

- **A streamed reply answers with its call's status.** A call that fails (not 2xx: a bad key, no
  credit, a limit) forwards nothing, and the reply is its answer, whole; it went out as a 200, so
  a page reading the stream showed nothing. Now the lowering wires the call's `sent.status` to
  the respond's status input, and the caller gets the 401 or 400 with the API's own error; the
  log, a replay and `holon verify` say the same. `chat-proxy`'s page shows such an answer ("the
  model's API answered 400: …").
- **A new sample, `gemini-chat`**: chat-proxy's streamed reply against Google's Gemini API (the
  Interactions API, `gemini-3.5-flash-lite`), whose free tier needs no credit. The answer goes to
  the page as it is written, the flow stores it once it ends, and a refusal (a bad key, the free
  tier's rate limit) shows on the page with its status. On the free tier Google may use what is
  sent to improve its products.
- **`respond` takes a computed status:** `respond body, status: req.json.code`, on the respond
  template's status input (`status_input: true`, there only when asked). A number out of 100–599
  answers 500.
- **A plain-HTTP host the server refuses says which.** A browser's password manager could fill
  the deploy settings' "plain HTTP to" with a saved login's name (it takes the field before the
  secret's value box for a username), and the field was then greyed out with it, so a deploy was
  refused with "this host never grants plain HTTP" and no way to see why. The refusal now names
  the host and the field, the field stays editable while it holds a host the server refuses, and
  the secret's value box tells password managers it isn't a sign-in. If yours filled it, its
  password may have gone into that secret: set it again from an empty box.

## 0.1.0-rc.23

Nothing to redeploy: no unit is hashed differently (the catalog only gains `channel_send`,
`channel_subscribe` and `http_stream`), and flows deployed with rc.22 keep running, their logs
read as they are.

- **Channels** (docs/L4s/channels-L4.md): packs a flow streams to whoever listens. A flow
  declares `channel updates`; `updates.send(x)` sends a pack once the run is over
  (`ChannelSend("updates")`), and a route that answers `respond updates.subscribe()` is an event
  stream of them (`ChannelRead("updates")`), `subscribe(from: "kept")` with the kept packs
  first.
  - A pack's id is its run's place in the log and its place among the run's packs (`12.0`), so a
    subscriber that comes back with `Last-Event-ID` gets what it missed, across a restart too;
    one that fell too far behind hears `event: missed`, and a paused, stopped or revoked
    deployment ends its streams with `event: closed`. Subscribers stay across a redeploy.
  - Limits, granted like a deployment's others and in the deploy bar: `max_pack_bytes`,
    `max_packs_per_run`, `channel_keep`, `channel_buffer` and `max_subscribers` (per channel).
- **Streamed replies.** `http.post(…, stream: true)` with `respond sent.body, stream: true`
  forwards the upstream's answer to the caller as it arrives (a model's tokens, a long export),
  with the upstream's content type and nothing buffered on the way; the flow still gets the whole
  answer, and the log keeps it, so the run replays. It takes `missing:`, not `status:` or
  `headers:`.
- **Pipes.** `updates.pipe(http.post(…))` sends a call's answer to the channel's subscribers piece
  by piece as it arrives, cut where a character ends, without an id and never kept; it asks
  `ChannelSend` beside the call's `HttpRequest`, and needn't be in a route.
- **A waiting run's progress goes out as it parks:** the packs it has settled that don't wait are
  sent then and logged with the parked part (`progress`, checked by `holon verify`), and the rest
  when it finishes, none twice.
- **Two new samples**, `live-board` (a board that updates as tickets arrive) and `chat-proxy` (a
  streamed `/chat` and a piped `/room` with a page that listens); `approval` now sends its order's
  status before and after the payment and serves it at `GET /orders/live`.
- **`holon verify` checks a run that computed with a secret.** A node fed a secret or a resume
  token can't be computed again from the log, which holds a placeholder; verify used to count
  such a run as not re-run, so an entry with its answer rewritten passed as the real one did.
  Now the outputs the log kept for that node stand in for it, as the editor's replay does, and
  the rest of the run is checked against them: the answer, the effects, the status. The report
  says which runs were checked that way and for how many nodes; what such a node produced is
  still trusted, and the checking-runs guide says so.
- **The admin page's browse…** chooses the folder to bind: the OS's own folder dialog when the
  editor is on the server's machine, otherwise a list of the server's folders to click through
  (`GET /admin/folders/browse`, admin-only). `HOLON_NO_FOLDER_DIALOG=1` keeps the dialog from
  opening. A listing that arrives late no longer replaces a path being typed, which is what failed
  CI after rc.22.
- **`holon serve` warns when it listens beyond loopback over plain HTTP**, naming which (the
  editor, the flows' ports); the manual says TLS ends at the operator's ingress or proxy.

## 0.1.0-rc.22

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.21 keep running.
A js node is now spelled `holon f(a) -> b js { … }`; the old `js f = (a) -> b { … }` is an error
that names the new form, and lowers to the same unit.

- **Named folders** (docs/L4s/named-folders-L4.md): files on the host that a flow reads and writes by
  name, as a collection's items, never seeing a path. A flow declares `folder inbox` and asks
  `FileRead("inbox")` or `FileWrite("reports")`; only an admin binds the name to a real folder,
  read-only or read-write, for the whole server or for one deployment (`holon serve --folders`,
  `holon folder add|list|remove`, the admin page). The server's own data can't be bound, a file
  name is checked before it reaches the OS, and its real path must stay inside the folder.
  - Reads are recorded and checked again before the run commits; writes, copies and deletes land
    when it finishes, written beside the file and renamed over it. A run whose read fails, or
    whose write doesn't land, answers 500 and performs nothing.
  - Files of any size: a handle (`logs.file(n)`: name, size, hash), a copy by handle, host to host
    (`archive[n] = h`), and ranges (`logs.lines(n, from: -20, limit: 20)`, `logs.slice(n, offset,
    length)`), kept in the log with what proves them part of the file's hash, so `holon verify`
    checks them without it.
  - What runs read is kept by content, in chunks, as long as a folder's `--keep` says (`bytes`,
    `hash`, `bytes for 30d`), and swept after (`holon folder sweep`); a folder may be capped
    (`--cap 10M`). `holon export --max-files-bytes N` or `--without-files` makes a smaller export,
    and `holon verify --files DIR` answers the reads it left out.
  - Create-only writes: `reports.create(n, text)` and `reports.create_copy(n, h)` never replace a
    file; a taken name fails the run.
  - A deployment's file limits, granted like its others: `inline_file_bytes` (1 MiB),
    `max_file_read_bytes` (64 MiB), `max_file_write_bytes` (1 GiB) and `max_files_per_run`
    (1,000). The error names the limit a run passed.
- **Runs on files.** `on file drop` runs a flow once for each file that arrives in a folder, once
  it stops changing, its handle the run's `ingress`; `deleted` runs on files removed, `matching
  "*.csv"` on some names, and a flow may have several lines, for several folders. The watcher
  acts on the OS's change notices and walks every folder every 30 s anyway. Files there at the
  deploy, and files a flow's own runs wrote or deleted, start nothing; files that arrive while
  it is down or paused run when it is back. `http.put(host, path, file: h)` sends a file as a
  request's body, streamed from the folder and checked against its hash, never through the flow.
- **Five new samples**: `inbox-report`, `log-archive`, `journal`, `drop-box` and `file-audit` for
  folders and runs on files.
- **`every` makes the clock a flow's trigger**: a flow on an interval that never reads `now` (a
  heartbeat that only counts) deploys and runs on the clock, where it couldn't deploy before.
- **The manual** (docs/manual): getting started, the concepts, a tour of the language and its
  reference, a generated page of every leaf, guides and reference pages. The editor has it in a
  docs tab, where each flow in it opens as an unsaved copy.
- A sample opened from the flows page deploys with its text, so its runs are explained by the
  names the text gives them; view mode's run pane resizes by dragging its top edge; the admin
  page says which list is empty.

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.20 keep running.

- **`respond` takes `headers:`**, a record of response headers: `respond page, headers: {
  "content-type": "text/html; charset=utf-8" }` answers a page instead of JSON, and `respond "",
  headers: { location: "/" }, status: 303` redirects. It is a third input on the template only
  when a flow asks for it, so a respond without it is the unit it always was (docs/LANGUAGE.md
  §5.11).
- **A new sample, `guestbook`**: a web page served by the flow itself. A js node renders the
  guestbook as HTML from the entries in a collection, with the clock wired in to say how long ago
  each was signed and what people type escaped; another parses the posted form; the flow stores a
  signature and redirects back to the page, or answers 400 to an incomplete form. Signed through
  a real browser, the page came back with the new entry.
- **Three things a js body may hold, fixed.** A regex literal: the lexer knows a `/` where a value
  can't be starts one, so the quotes in `/[&<>"']/g` no longer open a string. A port called after
  a leaf (`clock`): the printer keeps it as written, where it came back renamed and the body broke.
  A `#` in JavaScript (a CSS colour): `fmt`'s comment pass leaves a js body alone, where it
  re-inserted the rest of the line as a comment.
- The e2e suite no longer trips over a disabled account that the editor's own poll signed out
  before the test's next step, which is what failed rc.20's CI run (its release built fine).

## 0.1.0-rc.20

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.19 keep running.
A log written by rc.20 has fields an older reader skips; a policy may now set `claim_wait_ms`.

- **Key claims** (docs/L4s/key-claims-L4.md). Two requests with one idempotency key, sent in
  the same instant, both called the processor: the optimistic commit and its retry kept one call
  per *run*, never one per *key*, because an outbound call leaves mid-run, before the host knows
  whether the attempt will be kept. Now a run about to send its first outbound request claims
  every collection key it read that the flow may write, re-checks those reads against the live
  state, and holds the keys until it commits, is discarded or parks. One that finds a read changed
  as it claims it runs again from the new state having sent nothing; one holding keys and needing
  another run's lets go, waits and runs again, so nothing deadlocks; a commit waits for foreign
  claims on what it writes. Nothing is declared and nothing is in the manifest: the claim is
  derived from the run's reads and the grant, and a run that sends nothing claims nothing, so a
  read-modify-write with no call (the `likes` sample) stays as it was. State cells are never
  claimed: a cell is one per deployment, and claiming it would serialize every calling run.
  `claim_wait_ms` (10 s by default, ceiling 60 s) bounds the wait; past it the run answers 503 as
  busy, naming the key. Verified live: the race that sent two now sends one, the second run
  waiting 907 ms and replaying the first's record.
- **Claims shown.** `runs` says "retried ×1" and "waited 681 ms for a key" on a line that has
  them. `explain` says under the read it protects what the run claimed, how long it waited and
  held the key, or that it found what it read had changed and ran again having sent nothing.
  `holon check` and `show` say, apart from what a flow may do, *while it runs: holds the keys it
  reads in "charges" from its first outbound call until it commits*, for a flow that calls out
  over a collection it both reads and writes.
- **A new sample, `idempotent`**: one charge per Idempotency-Key. The same key with the same body
  answers the first charge again, the same key with a different body is refused (409), a key
  expires after a day, and the processor is called only when there is no prior charge, since
  `prior ?? fresh` never asks for the call on a replay.
- **A new sample, `tally`**: post a CSV and get each column's kind, the min, max and mean of a
  number column, the distinct values and top three of a text column; parsing text into structure
  is a js node's work, and the flow around it keeps the last summary in its state.
- The js engine's cost measurement (`what_a_call_costs`, on request) prints wall time too, cold
  and warm, and says which build it ran on.

## 0.1.0-rc.19

Flows deployed with rc.18 keep running. A flow with a `js` node redeploys as a new version
(the engine module was rebuilt for `check`), with nothing new to approve; a flow saved by rc.18
opens as before and saves smaller.

- **A js node that gives nothing says why.** `explain` names the engine's reason under the
  statement that placed the node: a body's exception as the engine words it ("`deep` gave
  nothing: TypeError: cannot convert 'null' or 'undefined' to object (the body's line 2, col
  19)"), or the engine's own failure ("out of fuel (200000000 instructions)"), for a node bound
  by name or inlined into `respond { … }`. The run records it beside the engine's step.
- **`holon check` compiles each js body** once, running nothing, and says a syntax error at its
  line in the file with the line and a caret, like any other mistake. A body that compiles passes;
  what it does at run time is the run's to say.
- **A flow document carries a module by engine and params, not by its binary.** The js engine's
  preimage is 3.2 MB (6.5 MB as hex) and each budget made another copy in every saved flow,
  deploy body and deployment entry; a flow with two budgets could not deploy at all (the control
  plane's 8 MiB body limit). A document now lists `{hash, engine, params}` under `modules`,
  rebuilt on open from the engine the build carries and checked by hash; a deployment entry for
  the `nearest` sample is 21 KB instead of 6.5 MB, and two budgets deploy.
- **Measured** (docs/L4s/js-node-L4.md F6): on a release build a trivial js body costs 6 ms, the
  `nearest` search 20 ms, a request through the deployed sample 50 to 60 ms more than one
  without a js node; a debug build is eighty times slower.

## 0.1.0-rc.18

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.17 keep running.

- **JavaScript as one node** (docs/L4s/js-node-L4.md, `docs/LANGUAGE.md` §5.8a). `js name =
  (inputs) -> outputs { body }` is a top-level definition beside `holon`, for the one
  computation the vocabulary can't spell. It runs on a JavaScript engine (Boa) that is itself a
  module: a fresh realm per call, no clock, no randomness, no timers and no network, so the same
  call gives the same answer. Budgets default to 200M instructions and 16 MiB per call (`fuel N`,
  `memory N` change them). `fmt` writes the statement back with its body as typed. Not yet: a
  body that throws, doesn't parse or runs out of fuel makes the node absent, but `explain`
  doesn't show the engine's message, and `holon check` doesn't parse the body. The engine
  rides in the editor's bundle, which grows from 1.9 MB to 6 MB.
- **Modules run** (docs/L4s/wasm-module-L4.md). Everywhere a flow runs (a request, a clock run, a
  resumed durable run, a deploy's first run, `holon verify`'s rerun, and design mode in the
  editor), a module runs under its budget. A deploy's manifest carries the budgets as
  `Limit(module_fuel=…)` and `Limit(module_memory=…)`, `holon check` says "run code it carries"
  with them, and the policy holds them to ceilings (`[ceilings]` raises them).
- **A new sample, `nearest`**: a dictionary that answers the closest word to a query by edit
  distance, the one computation a `js` node does while the flow around it stores, routes and
  answers.
- **The text and the canvas, linked by expression.** The cursor in a flow's text picks the node
  under it (`base64` in `base64(uppercase(req.body))`, not the whole line), and a node picked on
  the canvas marks the expression it is written as. It works in view mode too, where the text
  also shows the run: what didn't run is dimmed (the values of a `match`'s arms not chosen), the
  step is marked as you step, and the values of what you named are written beside it. On a
  window 1600px wide or more, the text pane is open by default.

## 0.1.0-rc.17

**Run `holon migrate` where you used to start `holon serve`.** A server keeps its state in
`~/.holon/data` now, wherever it is started from, so one started as before begins empty; the
old `.holon-*` files are left where they are, and `holon serve` says so when it sees them.

- **Everything in one `~/.holon`** (or `HOLON_HOME`): `data/`, `keys/`, `run/`, `tokens.json`,
  `mcp-hosts.toml` and `bin/`. `holon serve` needs no `--data`; it prints the folder it uses.
  `--data`, `HOLON_DATA` and each per-file flag still win.
- **`holon migrate`** moves what an older release left elsewhere: a server's `.holon-*` files
  (from the current folder, or `--from DIR`) into `~/.holon/data`, and key files from
  `~/.config/holon/keys` into `~/.holon/keys`. It says what it will do and asks (`--yes` doesn't
  ask, `--check` exits 1 when there is something to do), refuses while a server is running,
  checks that the secrets open with their key before anything moves, and copies and checks
  before it removes.
- **A secrets file records which key it uses** (format 3, `key_id`), so a data folder can be
  moved or restored from a backup and its secrets still open. A format 2 file is upgraded on
  open, keeping its key where it is.
- **`uninstall --purge` removes all of `~/.holon`**: data, key, run files, tokens and the list
  of agent hosts set up.
- **A module is a unit** (docs/L4s/wasm-module-L4.md), underneath only: a WebAssembly body with
  no imports, admitted by its exports and budgets. No host runs one yet, so nothing a flow does
  changes, and no unit already encoded is hashed differently.

## 0.1.0-rc.16

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.15 keep running.
What changes for them is what runs (below): a request in a branch not taken is no longer sent.

- **A branch not taken doesn't run.** `guard` and `coalesce` evaluate only the inputs they need:
  `if c then a else b` runs `a` only when `c` is true and `b` only when it isn't (or `a` has no
  value), `a when c` runs `a` only when `c` is true, and `x ?? y` runs `y` only when `x` has no
  value. So a request in an untaken branch isn't sent, an `await` there doesn't park the run, a
  read there isn't made, and a holon there isn't entered. No value changes. A choice that waits on
  an answer waits a round instead of deciding early. `select` stays strict and runs both sides,
  and a holon's inputs are still computed before it is entered. View mode dims the nodes a run
  didn't run, and the Inspector names the choice that skipped them. New sample: `cache-aside`.
- **`match`: a choice by value.** `match op { "add" -> a + b, "div" -> a / b, else -> 0 }`
  chooses the arm whose pattern equals the subject. Only that arm runs, and if it has no value
  the match has none (so `respond … missing: 400` answers 400), where a chain of `if` would fall
  through to its `else`. Patterns are literals; an arm may take several (`"sub", "minus" ->`).
  It lowers to `equals`, `guard` and `coalesce`, and `holon fmt` prints those back as `match`.
  `match` is now a keyword. New sample: `calc`.
- **`range(n)`** is the list `[0, …, n-1]`, for a whole `n` up to 10000 (nothing otherwise), so
  a `map` can count and still ends. New sample: `tickets`.
- **`holon verify` checks runs that made requests.** It answered a run's requests only from the
  earlier parts of a durable run, so any run that sent a request failed to check.

## 0.1.0-rc.15

**Delete the flows you deployed before this release, then deploy them again.** Some units are
hashed differently from this release on (below), and a server that finds a deployment stored under
the old hash refuses to start:

```
holon: deployment "…" in "…deployments.json": store error: Corrupt { … }
holon: refusing to start rather than overwrite it — fix the file or move it aside
```

To delete them, stop the server, move `deployments.json`, `deployments.logs` and
`deployments.activity.jsonl` out of its data directory (`~/.holon/data` unless you gave `--data`),
and start it again. Accounts and saved flows stay; open each saved flow and deploy it again. Their
runs' logs go with the old deployments.

- **Same-hash children are told apart by where their values go.** Two identical nodes in one
  route used to be ordered by a search with a cap, which could fail on a large route. Each child
  is now ordered by a hash of the path its values take to the route's outputs; a composite without
  such twins keeps its hash. Two identical nodes whose values go nowhere are an error: "the same
  value is computed twice and never used, so nothing tells the two apart: remove one, or use it".
  This is the change that re-hashes units.
- **A lambda uses what is around it.** `map(keys(r), fn: fn(k) -> r[k])` now compiles: a name a
  function uses from the route or holon it is written in becomes an input of the function, and
  `map` and `fold` pass it after the item. The explicit form, `map(xs, r, fn: fn(k, r) -> r[k])`,
  is the same unit. A function of the wrong arity is said at the function. `holon fmt` prints
  one-expression functions back as lambdas. New sample: `quote`.
- **A read inside a mapped function is answered.** A collection read, an outbound request or a
  waiting point inside a `map` or `fold` function came back absent, silently. Each item's is now
  answered, recorded and replayed on its own, and a round sends every item's requests together.
  View mode replays such a `map` from what the run recorded. New sample: `basket`.
- **`holon mcp` reads back what is there:** `deployments`, `flows` and `save` tools, and a run
  records who sent it.
- A wiring-form file whose unit hash starts with a digit reads again.
