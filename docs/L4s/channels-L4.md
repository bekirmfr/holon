# Channels: packs a flow streams to whoever listens, through the host (L4)

**Status:** built, all three slices (2026-10-26). The design below was written on 2026-10-25,
before any code, its claims checked against the code at `21f8ed1` and cited by file and line
there; §10 says where what was built differs from it, and the samples (`live-board`,
`chat-proxy`, `approval`) and `docs/LANGUAGE.md` are the reference for the language as it is.

A flow asks the host for a **channel**: a named sink, owned by the host, that the flow sends
**packs** into and that callers subscribe to. A pack is one Holon value. Subscribers receive each
pack as it is sent, over server-sent events, for as long as they stay connected. A request's own
answer can stream too: its **reply** goes to that one caller, and a streaming source (an upstream
that answers piece by piece) is forwarded into it as it arrives.

```
flow live-board
port 8126
collection tickets
channel updates

route POST /tickets (req) {
  tickets["{now}"] = { title: req.json.title, at: now }
  updates.send({ new: req.json.title, at: now })
  respond { ok: true }, status: 201
}

route GET /live (req) {
  respond updates.subscribe()
}
```

This is the step before a model on the host's GPU: a language model answers token by token, and a
flow that calls one needs to pass the tokens on as they come. Channels give that a shape that
keeps a run's graph whole, its writes atomic and its log checkable.

---

## 1. What is already there

**How a run runs.**

- **A run is rounds** (`holon-host/src/rounds.rs`, `run_rounds` at 211). Each round evaluates the
  whole flow; an `http_request` without an answer yet is **pending**, not absent
  (`Answer::NotYet` → `Slot::Pending`, `holon-core/src/eval.rs` 20–24, 97). A node that needs a
  pending input is pending without running, and a choice whose deciding input is pending waits
  (eval.rs 23, 288, 414–416), so a value a round produces is final *for that attempt*: fallbacks
  never decide early. Between rounds the host performs what is pending (`perform_all`, 380) and
  evaluates again.
- **Only the round that settles performs anything** (rounds.rs 244–251): when nothing is pending
  and nothing waits, its outputs' effects go to the `Host`. A run past its request or time budget
  returns no outputs at all (rounds.rs 232–237, 254–262).
- **Effects are values.** A sink builds a `Value::Effect`; `perform_effects`
  (`holon-host/src/lib.rs` 113) hands the root's effects to the `Host`, a root list of them in
  order (the computed fan-out of `map`, lib.rs 107–112), deriving each capability from the sink's
  name. An effect nested inside a record is not performed. The server's host is `RecordingHost`
  (`holon/src/firing.rs` 14), which only records them.
- **The commit applies what was recorded** (`commit`, firing.rs 233): state writes, then
  collection puts and deletes, then file writes, in the order made.
- **A run that loses a race runs again** (`fire_with_state`, firing.rs 110; up to
  `OPTIMISTIC_ATTEMPTS` = 4, 97): it ran on a copy and commits only if what it read is unchanged
  (`still_as_read`, 204); otherwise it runs again with its requests answered from its own record
  (`prior`, 151–183). A run with no state cell never commits: it returns from its one attempt
  (firing.rs 140).
- **A waiting durable run performs nothing** (rounds.rs 245: "A run still waiting performs
  nothing yet"); the server parks it (`durable::park_if_waiting`, called from
  `holon/src/listeners.rs` 116) and runs it again from the start when its event comes.

**How a request is answered.**

- **The data plane is `hyper` on `tokio`** (`holon/src/dataplane.rs`): `answer` (142) reads the
  request within its limits, runs the flow on the blocking pool (`spawn_blocking(serve_data)`,
  167–170) and sends the answer whole (`to_hyper`, 176: a `Full<Bytes>`), so nothing reaches the
  caller before the run is over. Connections are capped per listener (`max_connections`, 1024,
  dataplane.rs 44, 54).
- **`serve_data`** (listeners.rs 15) adds CORS for the flow's `browsers` to whatever it answers
  (22–45) and calls `serve_flow` (57), which answers 503 unless the deployment is running (72–74)
  and runs the route through `data::handle` (`holon/src/data.rs` 69).
- **The answer is the route's output 0, read by `data::respond`** (data.rs 137): a record with a
  numeric `status` is the envelope `{status, headers, body}`; a text body goes out as
  `text/plain`, any other as JSON, unless `headers` set a content type. `DataResponse` is
  `{status, headers, body: Vec<u8>}` (data.rs 27).

**How a run calls out.**

- **The request a node makes** is `{method, url, headers, body}`, with `url` always
  `https://host/path` (`holon-core/src/primitives.rs` 728–735). `http_request` has frozen
  `method` and `host` params and `path`, `headers`, `body` inputs (`holon-core/src/catalog.rs`
  303–307). `http.post(host, path, headers: h, body: b)` lowers to it (`lang/lower.rs`
  1360–1384), named arguments becoming inputs or params by the leaf's interface (`unit_call`,
  1437).
- **The performer has no node path:** `Performer::perform(&self, request, limits) -> Value`
  (rounds.rs 95). `perform_all` knows each request's path as it spawns it (rounds.rs 400–411)
  and records an `Exchange {path, request, response, start_us, end_us}` (101).
- **`Outbound`** (`holon/src/outbound.rs` 124) is the real performer: granted hosts only, HTTPS
  unless the host was granted plain HTTP (then it calls `http://`, 112–119), no private
  addresses unless allowed (33–36). It reads the whole response body at once, within
  `max_response_bytes` (outbound.rs 254–258).
- **Bytes a flow doesn't need never pass through it:** a file sent by handle (`http_upload`) is
  read from the folder as it is sent (outbound.rs 15–16, 184–190).

**How a run is kept.**

- **The log** (`holon/src/recorder.rs`): one JSONL file per deployment
  (`<deployments>.logs/<slug>.jsonl`, `file_for`, 343), read back at open with its chain checked
  and trimmed to the last `--keep-runs` entries (500 by default; `Log::open` and `open_keeping`, 350–380). An entry
  (`LogEntry`, 54) has the run's `performed` effects (`PerformedEntry {capability, payload}`,
  `holon/src/wire.rs` 111), its `rounds` of `LoggedExchange {path, request, response, start_us,
  end_us}` (recorder.rs 206), its outputs and its status.
- **`holon verify`** runs an entry again and compares its outputs, its `performed` effects as
  JSON and its status with the log's (`holon/src/verify.rs` 7–9, 425–438).
- **`explain`** reads the exchanges from the entry's rounds (`holon-author/src/agent.rs` 477).

**How a flow names a host resource.**

- An addressed capability per use: `CollectionWrite(name)`, `FileRead(name)`, `SecretRead(name)`
  (`holon-core/src/kind.rs` 133–142), derived from the leaf and its param (`capabilities_of`,
  kind.rs 287–309), each a `Kind` (`kind`, 166–172: `Sink` for what commits irreversibly).
- A header names the resource: `collection notes`, `folder inbox` (`lang/parse.rs` 277,
  `HeaderKind::Collection`/`Folder` in `lang/ast.rs` 58–60). The lowering keeps the declared
  names (`Store`, `COLLECTION`, `FOLDER`, `lang/lower.rs` 100–135; `store`, 612) and turns
  `name.method(…)` into that store's leaf (`method`, lower.rs 1354; 1404–1426).
- **Consent** is the deploy's (`holon/src/deploy.rs` 121–140): the manifest is what the graph
  derives plus what the author declares; the policy refuses what it never grants, grants what it
  grants without asking (`[grants] auto`, with `(*)` patterns), and asks a person for the rest.
  An agent's deploy that needs a person's approval is refused with 403 (deploy.rs 136–140).
  `describe_item` says each item in words (`holon-author/src/describe.rs`, the `named` table at
  19). Limits a deploy may raise are `LIMIT_NAMES` (`holon/src/permissions.rs` 35, 19 of them),
  defaults in `holon.toml`'s `[limits]` and caps in `[ceilings]`.
- **A deployment** (`struct Deployment`, `holon/src/main.rs` 340) has persisted fields and
  derived ones (`server`, `permit`: "derived at deploy and load, never persisted"); a redeploy of
  a slug keeps its state and sessions (`deploy.rs` 188–194). Its status is running, paused or
  stopped (`holon/src/lifecycle.rs` 26).

So the new parts are: a sink performed *after* the commit, toward listeners outside the run; a
registry of channels in each deployment; an answer that can be a stream; and a performer that
can forward a body as it reads it.

## 2. What a channel is, and when a pack leaves

A channel belongs to a **deployment**, a derived field of `Deployment` like `server` (rebuilt at
deploy and load, never persisted), so a redeploy keeps it and its subscribers as it keeps the
slug's state. It has:

- a **sequence**: each pack gets the next number, its SSE `id`;
- the **kept packs**: the last `channel_keep` (100 by default), for a subscriber that joins late
  or reconnects;
- its **subscribers**: each a bounded queue (`channel_buffer`, 256 packs) feeding one connection.

**Packs are sent when the run is over and its writes have landed, in the order the run made
them.** Four facts of §1 decide it (fork 1, §9):

- only the settling round's effects are performed (rounds.rs 244–251), so a run has packs only
  once it settles;
- an attempt that loses its race is discarded and run again (firing.rs 151–183): a pack sent from
  it would announce a run that didn't count, with values read from state that changed;
- a run can fail after its packs are computed: past its budget (no outputs, rounds.rs 232–237),
  or with writes that didn't land (`not_all_written`, firing.rs 195): a pack would announce a
  write that never happened;
- a waiting run performs nothing (rounds.rs 245).

Where: in `fire_with_state`, once the run is final and the state lock is released: after the
commit on the optimistic path (firing.rs 162–170) and the locked one (185–189), and on the
stateless return (140), which has no commit but is final too. A run whose writes didn't all land
sends none.

So a channel streams **across runs**: each run that finishes adds its packs, live, to every
subscriber. Streaming **within one run** comes from a streaming source (§4), whose bytes the flow
doesn't compute.

## 3. The language

A channel is declared like a collection, has one method, and is subscribed to by answering with it:

| Written | Lowers to | Means |
|---|---|---|
| `channel updates` (header) | `HeaderKind::Channel("updates")` | a channel of this flow |
| `updates.send(v)` | `channel_send{channel: updates}(v)` | `v` as one pack, sent when the run is over (§2) |
| `respond updates.subscribe()` | `channel_subscribe{channel: updates}` as the respond's body | the answer is a subscription: an SSE stream of the packs |
| `respond sent.body, stream: true` | `respond`, its body `sent.body` of an `http_stream` call | the answer is the call's bytes as they arrive (§4) |
| `updates.pipe(http.post(…))` | `http_stream{pipe: updates}` | the call's bytes to the channel's subscribers as they arrive |

(The table as built; §10 lists what changed from the design, the spelling first.)

**In the author crate:** `channel` joins the header words (parse.rs 277) and `HeaderKind`
(ast.rs), and the lowering's declared names (lower.rs 612) as a third store-like receiver whose
only method is `send` (lower.rs 1404–1426, beside a collection's and a folder's); `channel` joins
`RESERVED` (lower.rs 77, print.rs 29) and the editor's `KEYWORDS` (editing.rs 57).

- **`send` is a statement or a binding,** as a collection write is (LANGUAGE.md §5.10): on its
  own line it is performed; inside a holon its effect leaves through a named output (`sent =
  updates.send(v)`), as `put = posts.put(k, v)` does. Inside `map` it sends one pack per item,
  in order: the fan-out `perform_effects` already flattens (lib.rs 107–112).
- **A pack is any value but an effect** (`holon_host::is_storable`, lib.rs 261, the rule state
  writes use), at most `max_pack_bytes` as JSON. It goes out as `id: <seq>` and `data: <JSON>`;
  a record with a text `event` field also names the SSE event (`event: ticket`), so a page can
  listen by kind.
- **`respond updates`** takes no body: the subscription is it. `channel_subscribe` builds an
  effect value that `respond` puts in the envelope as `stream`; since `perform_effects` performs
  only root effects (lib.rs 113–130), it isn't performed as a sink, and `data::respond` (data.rs
  137) is what reads it: a stream of the channel, with status 200, `content-type:
  text/event-stream`, `cache-control: no-cache`, and any `headers:` given. The respond template
  gains a `stream` param, absent unless written, so every respond so far is the unit it was (the
  way `headers` was added, `holon-author/src/templates.rs` 110).
- **Who may subscribe is the route's to decide,** with what it checks of the request, as for any
  route; `browsers` lets pages on other sites listen, since `serve_data` adds CORS to every
  answer (listeners.rs 22–45).
- **The subscription opens after the route's run is over,** so a route can check, store or count
  before it answers with the stream.

### Consent

- `ChannelSend(name)`, from `channel_send`: "send packs on its channel "updates"".
- `ChannelRead(name)`, from `channel_subscribe`: "let callers subscribe to its channel "updates"".
- Both are addressed capabilities in `kind.rs` (133–142), derived in `capabilities_of`
  (287–309), of `Kind::Sink` (166–172: the host does something irreversible), never sourced, and
  said by `describe_item` (describe.rs 19). They are granted as any capability is (deploy.rs
  121–140): automatically where the policy says (`ChannelSend(*)`), else by a person.
- A streamed reply asks for nothing new: `NetworkIngress` answers the request, and the source's
  own `HttpRequest(host)` (kind.rs 309) covers the call.

## 4. Streaming sources and the reply

A **streaming source** is a leaf whose answer arrives in pieces. In the first slice that is
`http_request` with `stream: true`:

```
route POST /ask (req) {
  sent = http.post("api.openai.com", "/v1/chat/completions",
                   headers: { authorization: "Bearer {secret.openai_key}" },
                   body: { model: "gpt-4o-mini", stream: true, messages: req.json.messages },
                   stream: true)
  history["{now}"] = { asked: req.json.messages, said: assemble(sent.body) }
  respond sent.body, stream: true, headers: { "content-type": "text/event-stream" }
}
```

- **`stream` is a new frozen param of `http_request`** (catalog.rs 303–307), absent unless
  written, so every request so far is the unit it was. `unit_call` already turns a named
  argument the leaf has as a param into that param (lower.rs 1437). It doesn't enter the request
  value (`{method, url, headers, body}`, primitives.rs 728–735): the request is the same; only
  how its answer is delivered differs.
- **The flow still gets the whole answer.** `sent` is the response record as today, its `body`
  the complete text within `max_response_bytes` (outbound.rs 254–258). Everything that uses it,
  the `history` write and an `assemble` js holon that joins the deltas, waits for the end as now.
- **The caller gets it as it comes.** When a route's `respond` has `stream: true`, its body must
  be wired straight from a streaming source's `body` (`lower.rs`'s `respond`, 631, says so
  otherwise: "only a streaming source's body can be streamed; this one is computed"). The host
  reads that from the deployed graph, as it reads `file_arrival`'s params for a trigger
  (`holon_core::file_triggers`, `holon-core/src/triggers.rs` 28): the path of the source node
  whose body the reply carries.
- **Forwarding happens in the performer.** `perform_all` (rounds.rs 380) knows each request's
  path (400–411), so it passes a forwarder to the request at the streaming path; `Outbound`
  (outbound.rs 254) reads the body in a loop instead of `read_to_end`, handing each piece to the
  forwarder as it reads it and keeping the whole for the answer. `Performer` gains a method that
  takes the forwarder; the others (`folders.rs` 1173, `verify.rs` 505) keep `perform`.
- **Status and headers are fixed before the first chunk.** In a streamed `respond`, `status:` and
  `headers:` must be constants (the lowering checks, as it checks that a host is a literal,
  lower.rs 1366–1370); `missing:` doesn't apply. If the upstream answers other than 2xx, nothing
  is forwarded, and the reply is sent whole at the end from what the flow answers.
- **The reply ends when the run is over:** its writes commit and its packs go out (§2), then the
  stream closes. A run that fails after the stream began can't change its status; the stream
  ends early (with `event: error` when the content type is `text/event-stream`), and nothing it
  wrote commits.
- **What the caller saw is the upstream's bytes, never the flow's computation.** It is the one
  thing shown before the run is over, and it stays true: an attempt that runs again answers the
  request from its record (`prior`, firing.rs 151–183), so the attempt that counts uses the bytes
  the caller already has.
- **One streamed source per reply.** Other `stream: true` requests in the route are performed
  like any other; only the one `respond` carries is forwarded.

The **model capability** (inference on the host's GPU, its own design) is a second streaming
source of the same kind: its tokens forward to the reply, and the flow gets the whole answer.

## 5. The data plane

- **An answer can be a stream.** `serve_data` (listeners.rs 15) takes a reply handle instead of
  returning one `DataResponse` (data.rs 27): a head (status, headers) once, then chunks, then the
  end. A route that doesn't stream sends the head and the whole body at the end, as now.
  `to_hyper` (dataplane.rs 176) builds a `StreamBody` over a bounded `tokio::sync::mpsc` in place
  of the `Full<Bytes>`; `answer` (142) awaits the head, not the run.
- **The registry.** Per deployment, each channel by name: its sequence, its kept packs, its
  subscribers. Sending (§2) assigns the next sequence, keeps the pack, and offers it to each
  subscriber's queue with `try_send`: a run never waits on a subscriber.
- **Slow subscribers are dropped.** A full queue gets a final `event: missed` and its connection
  is closed; the browser's `EventSource` reconnects with `Last-Event-ID`, and if what it missed
  is still kept, it gets that first.
- **Late joiners.** With `Last-Event-ID`, the kept packs after it; without, only new ones.
  `respond updates, from: "kept"` sends every kept pack first: a page that shows the last hundred
  on load.
- **Liveness.** A comment line every 15 s keeps proxies from closing a quiet stream. Pausing or
  stopping the deployment ends its subscriptions with `event: closed` (a new subscribe already
  gets 503, listeners.rs 72–74); redeploying doesn't.
- **Connections.** Subscriptions are long-lived, so they have their own cap, `max_subscribers`
  per deployment, past which a subscribe answers 503; they count toward the listener's
  `max_connections` (dataplane.rs 44) as any connection does.
- **After a restart,** the kept packs are rebuilt from the deployment's log, read back at open
  (recorder.rs 350–380). That reaches as far as the log keeps runs (`--keep-runs`, 500 by
  default): a channel keeps fewer packs after a restart when its last `channel_keep` packs were
  sent by more runs than the log keeps.

## 6. The log, replay and verify

- **A pack is an effect,** logged in `performed` as `{capability: "ChannelSend(\"updates\")",
  payload}` (wire.rs 111), so `holon verify` recomputes it and compares it as it compares every
  effect (verify.rs 425–438): a pack is checkable output.
- **Its sequence number is logged beside, not inside:** a new `LogEntry` field, `packs:
  [{channel, seq}]` in the order of the run's `ChannelSend`s. Inside the payload it would break
  that comparison, since a re-run can't know the number.
- **A subscription is logged as its route's run** (status 200, `stream: "updates"`); which packs
  each subscriber received is the data plane's, as any response on the wire is.
- **A streamed exchange** is a `LoggedExchange` (recorder.rs 206) as today, the whole response
  included, plus `chunks` and `first_byte_us`. Replay answers from the record (`Recorded`,
  rounds.rs 282) and streams nothing; `explain` (agent.rs 477) says "streamed 412 chunks to the
  caller over 3.2 s".

## 7. Limits

New `LIMIT_NAMES` (permissions.rs 35), with defaults under `[limits]` and caps under
`[ceilings]`, asked for in the manifest when raised:

| Limit | Default | Ceiling | Bounds |
|---|---|---|---|
| `max_pack_bytes` | 64 KiB | 1 MiB | one pack, as JSON |
| `max_packs_per_run` | 1000 | 100,000 | packs one run sends (past it, the run fails before anything commits, naming it) |
| `channel_keep` | 100 | 10,000 | packs kept per channel for late joiners |
| `channel_buffer` | 256 | 4096 | packs queued per subscriber before it is dropped |
| `max_subscribers` | 100 | 10,000 | open subscriptions per deployment |

A streamed reply is held to what holds a run now: `run_budget_ms` (the whole run, rounds.rs
`Limits`, 34) and the source's `max_response_bytes`.

## 8. Samples and slices

1. **Named channels.** The header, `send`, `respond updates`, the registry, kept packs,
   `Last-Event-ID`, dropping slow subscribers, the limits, rebuilding from the log. Sample:
   **`live-board`** (port 8126; the samples use 8101–8125, but for 8104 and 8118, and 8130–8131): `POST /tickets` stores
   a ticket and sends a pack; `GET /live` subscribes; `GET /` is a page (a js holon renders it)
   whose `EventSource` adds each ticket as it arrives. Run live with two browsers; deployed by
   e2e (`editor/e2e/samples.e2e.ts`), which subscribes, posts, and reads the pack.
2. **Streamed replies.** `stream` on `http_request` and on `respond`, the forwarding performer,
   the reply handle, the lowering's checks, the logged chunk timing. Sample: **`chat-proxy`**
   (port 8127): `POST /ask` forwards a chat to an OpenAI-compatible API with `stream: true`,
   streams its tokens to the caller, and stores the assembled answer. Run live against a local
   model (`127.0.0.1:11434/v1`: the flow asks for `PlainHttp` and private addresses, exceptions a
   host grants, outbound.rs 33–36, permissions.rs 405) or against a hosted API with a secret.
   e2e has no upstream to call today: the slice adds a stub to its fixtures (`editor/e2e/`) that
   streams a fixed answer slowly, on loopback, with the e2e policy granting plain HTTP and
   private addresses to it.
3. **Progress from durable runs, and piping.** A waiting run sends the packs settled before its
   `await` when it parks, and never again on resume (each `send` node's pack recorded with the
   parked run by path); `updates.pipe(source)` forwards a streaming source's chunks to a named
   channel as they arrive (a shared live transcript).
4. **Out of scope here:** a flow triggered by another's channel (an event bus between flows),
   WebSockets, and the model capability, which gets its own note and builds on slice 2.

## 9. Forks, surfaced

1. **Packs when the run is over, or as soon as computed?** When it is over (§2). A settled value
   is final within its attempt (eval.rs 20–24), but the attempt may be discarded, fail, or park
   (§2's four facts), and a pack can't be taken back. The cost: one run's packs arrive together
   at its end. Streaming within a run is what streaming sources are for, and slice 3 adds
   progress for the one case, a parked run, whose end is far away.
2. **Server-sent events or WebSockets?** SSE: one direction is the need, it is plain HTTP through
   any proxy, browsers have `EventSource` with reconnection built in, and `Last-Event-ID` gives
   catching up. A WebSocket is for receiving on the same connection; a route already receives.
3. **`respond updates` or `updates.subscribe()`?** Answering with the noun: it says what the
   caller gets, and it is the one place a channel is a value.
4. **Where kept packs live:** memory, rebuilt from the log (§5), not a store of their own. The
   log holds every pack, chained and checkable; the price is that a restart keeps only what the
   log keeps.
5. **A channel per deployment or per version?** Per deployment, as state is (deploy.rs 188):
   subscribers shouldn't drop on every redeploy, and the channel's name is in the flow's text,
   which a redeploy keeps.
6. **Where a pack's sequence is logged:** beside `performed`, not in it (§6), so verify compares
   packs as it compares every effect.

## 10. As built

Where the code differs from the design above, and why.

- **The spelling is `respond updates.subscribe()`** (fork 3 went the other way): a method on the
  channel reads as what it does, and `send` and `subscribe` are then one family. Its option is an
  argument, `subscribe(from: "kept")`. `updates.subscribe()` anywhere but as a route's answer is an
  error that says so.
- **The subscription is the respond envelope's body,** not a `stream` param: `channel_subscribe`
  builds an effect value, `data::respond` finds it as the body (or the whole output) and the data
  plane streams it. The respond template is unchanged, so no flow's hash moved.
- **A pack's id is its run's log seq and its place among the run's packs** (`12.0`), not a
  per-channel sequence, and no `packs` field is logged: the id is recomputed from the entry, so
  the kept packs rebuild from the log with the ids subscribers saw, and they only grow, across
  restarts too. Packs go out in `log_run` (firing.rs), as the entry is pushed, from the logged
  (redacted) `performed`.
- **`max_subscribers` is per channel,** not per deployment: one busy channel can't starve another.
- **The SSE `event` field from a record's `event` text is not built:** every pack is a `message`
  (`id:` and `data:`), and the host's own ends are `event: missed`, `event: closed` and
  `event: error`.
- **A streamed call is its own leaf, `http_stream`, not a `stream` param on `http_request`:** the
  builder writes every catalog param into every node it places, so a new param would have
  changed the hash of every flow with a call. `http.post(…, stream: true)` lowers to it (`stream:`
  last); its request is `http_request`'s plus `stream: true`, which tells the performer
  (`folders::Sender`) to forward it through the run's `outbound::Forwarder`, once per run.
- **A streamed reply is 200 with the upstream's content type,** `cache-control: no-cache` and
  `x-accel-buffering: no`; it takes no `status:` or `headers:`, but takes `missing:`, the answer
  of a run whose call was never made (an input absent). A non-2xx upstream forwards nothing and
  the reply is sent whole at the end, as the design says.
- **Chunk timing is not logged** (no `chunks`, `first_byte_us`): the exchange is logged whole, as
  any call's, and replays.
- **A waiting run's progress (slice 3)** is the sends among its settled outputs when it parks,
  logged with the parked part as `progress` and checked by `holon verify`; a later part sends only
  what an earlier part didn't, matched one for one by channel and JSON (the parts of a run replay,
  so the same sends settle again, the same), not recorded by node path.
- **`updates.pipe(call)` (slice 3) is `http_stream` with its `pipe` param set:** one leaf, which
  asks `ChannelSend(updates)` beside `HttpRequest(host)`. Its pieces are text packs without an id,
  never kept and not counted as the run's packs: what a late subscriber gets is what the run sends
  once it is over. A piped call needn't be in a route.
