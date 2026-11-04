# Inbound WebSockets: a connection whose every message is a run (L4)

**Status:** design, before any code. Written 2026-11-04 against `450e3de`; re-checked the same day
against `main` as merged into this branch (`ff03a80`), after the request path's work landed, and
cited by file and line there. Each block takes the request path's log levels (C5, §5 here).

This revises [host capabilities](host-capabilities-L4.md), which filed inbound WebSockets as
"later" (§5's table, the end of §5.9, Tier 3 in §7): "channels (SSE) and a POST cover what a
browser sends and hears". Decided 2026-11-04 that they are fundamental: the libraries web apps
are built on (collaborative editing, GraphQL subscriptions, presence) expect one two-way
connection, not two half-connections. It is Tier 1, step 1.9, on track D beside 1.6, whose
engine it shares (host capabilities §7, revised the same day).

It follows host capabilities §5.9's rules for every connection that outlives a run: **the host
owns the connection; a run sees whole messages, never bytes off the wire; what a run received and
sent is its input, logged; the connection's own life is outside every run, in its own record**.
§9 answers that note's §4 checklist. It builds on [channels](channels-L4.md) (packs to
subscribers) and [triggers](triggers-L4.md) (a flow with several handlers).

**Names.** Host capabilities §5.9 already uses *socket* for a bound connection to a payment
network (`holon socket add`, `SocketUse("processor")`, `on message processor (m)`). So this is
`websocket`, its item `WebSocket(path)`, and its handlers are blocks inside the statement, never a
top-level `on message`.

A flow declares a **websocket**: a path a browser opens a WebSocket on. What the flow does
happens in runs, as for requests:

- **open** is a run when a browser connects. It decides whether to accept, and what the
  connection hears (channel subscriptions).
- **message** is a run per message the browser sends. Its `respond` goes back to that connection
  only.
- **close** is a run when the connection ends, whichever side ends it.

Nothing goes out on a connection except a run's reply or a pack on a channel it subscribed to:
both are recorded effects of recorded runs. So a connection's whole conversation is in the log,
and every run in it replays as a request's does.

```
flow room
port 8132
channel room
collection people

websocket /room/:name {
  open (req) {
    people[req.conn] = { name: req.params.name, since: now }
    room.send({ joined: req.params.name })
    respond room.subscribe()
  }
  message (msg) {
    who = people[msg.conn]
    room.send({ from: who.name, text: msg.text })
  }
  close (c) {
    delete people[c.conn]
    room.send({ left: c.params.name })
  }
}
```

`check` says:

```
a server on :8132
  WebSocket /room/:name
it may:
  read the clock
  hold WebSocket connections on /room/:name, and run on each message
  read items of its collection "people"
  put and delete items of its collection "people"
  send packs on its channel "room" to whoever subscribes
  let callers subscribe to its channel "room"
```

---

## 1. What is already there

- **Channels** (built) are the push half: a flow sends packs from any run, the host delivers them
  to subscribers after the run commits, keeps the last `channel_keep`, drops a slow subscriber,
  and caps subscribers per deployment (`channels.rs` 39–50, 164). Today the only subscriber is a
  server-sent-events response.
- **Routes** run once per request (`listeners.rs`, `serve_data`) under the deployment's grant,
  with claims ordering runs that touch the same keys. Every request is logged (255). A reply that
  carries a secret needs `SecretAnswer` (`listeners.rs` 188–196).
- **Each handler has a log level** (request-path C5, built): `strict` by default, its record on
  disk before its answer (about 0.6 ms of fsync, with group commit, C2 e); `lazy`, on disk within
  about a second; `unrecorded`, counted and never logged, failed or not (resilience-audit §2.8),
  approved by a person. Only a handler that may record lightly takes a word: every capability
  sourced, none `Await` (`may_record_lightly`, `declare.rs` 86).
- **What starts a run** is one item per kind (host capabilities §1): a route `NetworkIngress`, a
  clock `Clock`, a file `FileArrival`, an action `Action`.
- **The data plane** is hyper http1 on tokio (`dataplane.rs`), each connection served by
  `serve_connection` (162) without upgrades. hyper 1 supports them (`.with_upgrades()`,
  `hyper::upgrade::on`). No WebSocket codec is in the tree (`Cargo.lock` has no `tungstenite`);
  host capabilities §5.9 names tokio-tungstenite for outbound feeds (step 1.6), so both directions
  share one engine.
- **There is no bytes value** (host capabilities §1). One is planned, `Bytes` (§5.10, step 1.1).
- **Browsers' `WebSocket`** can't set headers: a page authenticates with a cookie or a token in
  the query string, both of which the opening request carries.

## 2. The connection and its runs

- **The opening request** is a GET with `Upgrade: websocket`. It matches the websocket's path like
  a route (`:params`, and `*rest` from static-files-L4), and runs **open** with the request record
  plus `conn`, the connection's id (random, chosen by the host, so it is in the record and the
  log).
- **Open's answer decides the handshake.** A subscription, or a list of them, accepts and
  subscribes. A record with `status` 4xx or 5xx refuses the upgrade with that status. Absent
  (`missing:`) is 403. Any other value accepts and is sent as the first message. The 101 goes out
  once open's run has committed, so its writes land before the first message is read.
- **Each text frame** runs **message** with `{conn, params, query, text}`, where `json` is
  `parse_json(text)`, lazily, as `req.json` is. A message's `respond` goes to that connection
  only; no `respond` sends nothing.
- **Messages on one connection run one at a time, in order.** The host queues them per
  connection, up to `socket_queue` (32). A full queue closes the connection with 1013 ("try again
  later"). Messages on different connections run concurrently, as requests do, and claims order
  them where they touch the same keys.
- **Packs** on a subscribed channel go out as text frames, each a pack's JSON, after their run
  commits, as for server-sent events. They interleave with replies in the order the host sends
  them.
- **close** runs once, after the last message's run, with `{conn, params, query, code, reason}`.
  It answers nobody.
- **Pausing or stopping** a deployment closes its connections with 1001, and each runs close.
  Redeploying doesn't close them: the next message runs the new version, as the next request
  would.

## 3. The connection's own record

Host capabilities §5.9 and fork 20: what isn't a run has its own record. Per connection, a log
hash-chained like a deployment's and kept as long as its runs: opened (when, from where, the open
run's seq), pings and missed pongs, frames refused (too big, binary before `Bytes`, over the
rate), the queue full, and closed (code, reason, by whom). It is synced as a `lazy` run's record
is (request-path C2 d): on disk within about a second. Pings are the host's (every 15 s; a
connection that doesn't pong within 30 s is closed with 1001), from nothing a run computed.
`holon log --conn <id>` shows a connection's runs and its record together, in order.

## 4. The language and what it lowers to

`websocket PATH { open (r) {…} message (m) {…} close (c) {…} }`. `open` and `message` are
required, `close` is optional. Each block is a route's block: statements, `respond` (not in
`close`), its own names, and the same optional word before its braces, `lazy` or `unrecorded`
(`message (m) lazy { … }`), under the same rule (§5).

Each lowers to an endpoint-shaped composite with frozen params `declare = "websocket"`, `path`,
`on` (`open`, `message` or `close`), and `log` when it isn't strict, as a route's carries
`declare = "endpoint"`, `method`, `path` (`declare.rs` 1–24) and `log` (C5). `read_server`
collects them into `Server.websockets`, by path, each with its handlers' hashes. A websocket's
path clashes with a GET route of the same shape: a path is either a page or a connection.

## 5. Consent

- **The item is the trigger:** `WebSocket("/room/:name")`, "hold WebSocket connections on
  /room/:name, and run on each message". Every kind of thing that starts runs has its own item
  (§1), and this one differs from a route in a way an approver should read: callers stay
  connected, and their messages start runs for as long as they do.
- Subscribing is `ChannelRead`, sending `ChannelSend`, as now. A reply carrying a secret needs
  `SecretAnswer`, as a route's does.
- **Log levels per block,** C5's words and rule unchanged. A block that only reads and replies
  may be `lazy` or, approved, `unrecorded` (labelled `WebSocket /room/:name message`, as the log
  and the tallies name it). One that sends packs or writes state is `strict`: its record is on
  disk before its reply and its packs go out. So is an `open` that subscribes, as long as
  `ChannelRead` isn't allowed to record lightly (static-files-L4 §10 has that fork).
- **Limits** a deploy asks for above their defaults, up to the host's ceilings, as
  `run_budget_ms` is: `max_websockets` per deployment (100), `max_message_bytes` (64 KiB),
  `messages_per_second` per connection (10, bursts of 20; past it, 1008).

## 6. The log, replay and verify

- **Every run is a log entry at its block's level,** `kind` `open`, `message` or `close`,
  `route` `WebSocket /room/:name`, with `conn` in its request record. An `unrecorded` block's runs
  are counted in its tally instead. A reply is the entry's
  `response_body`; a pack is in its sending run's `performed`, as now.
- **Replay and verify** are a request's: the record, the state as the run began, the clock, the
  answers. A connection's order is the order of its entries.
- **Volume is the price:** a chat at ten messages a second writes ten entries a second, unless a
  person approved its block `unrecorded`.

## 7. Limits that stay

- **Latency per message is a run's.** A `strict` block (any that sends packs or writes) waits
  for its record's fsync, about 0.6 ms, shared with other runs by group commit (request-path
  C2 e); a `lazy` one costs tens of microseconds over its run. One connection's messages run one
  at a time, so a strict block takes at most about 1,600 messages a second from one connection.
  Fine for chat, presence, notifications and collaborative edits at typing speed. Not for 60 Hz
  game state or cursor streams; those want the browser to batch.
- **No memory per connection except state.** What a connection is (who signed in, which room) is
  kept in a collection or state keyed by `conn`, so it is recorded. There is no hidden
  per-connection variable, by design.
- **Inbound messages are at most once.** A message whose run hadn't committed when the server
  stopped is gone with the connection. The client sees the close and resends, as it must over any
  WebSocket.
- **One machine:** a connection lives on the machine it opened on, like everything in Holon today.

## 8. Binary frames

Refused (1003) until `Bytes` lands (host capabilities §5.10, step 1.1). Then a binary frame is
`msg.bytes`, a `Bytes` value within `max_message_bytes`, logged as the boundary rules there say
(base64 in the log line, large ones in `blobs.rs`), and a reply or pack that is `Bytes` goes out
as a binary frame. Nothing here needs a second representation of binary data.

## 9. Host capabilities' checklist (§4)

1. **The item and its words:** `WebSocket(path)` (§5).
2. **The resource:** none bound; the flow's own port, as routes. No credentials.
3. **Reads are the run's input:** each message is its run's request record, logged (§6).
4. **Effects land when the run finishes, at most once:** a reply and packs go out after the
   commit, and after the record is on disk for a `strict` block. A run that runs again (claims)
   sends its reply once, from its last attempt, and its packs once, as channels already do.
5. **Sharing:** claims and locks, as for requests. A connection lives on one server.
6. **Triggers:** a message is whole when its last frame arrives (the settle rule). It isn't run
   twice: one connection's queue hands each message to one run (the key is `conn` and the
   message's place in it). After the run, nothing: it isn't acknowledged to the client unless the
   flow replies.
7. **Secrets:** a reply as a route's answer (`SecretAnswer`); a pack as channels rule (refused
   when computed from a secret).
8. **Limits:** §5, each with a default and a ceiling, shown in the deploy bar.
9. **Verify:** re-runs every open, message and close run; the connection's record is checked by
   its chain, not re-run.
10. **Seeing it:** `holon check`'s words; the deploy bar; the deployment's page showing open
    connections, their age and their queue; the manual's permissions page and a guide.
11. **The sample:** `room` (§11), deployed by the e2e tests with the browser as the client. No
    outside service is involved, so no stand-in is needed.
12. **The engine:** tokio-tungstenite on the data plane's runtime, shared with step 1.6's
    outbound feeds. Pure Rust, rustls where TLS is involved (it isn't here: TLS ends at the
    ingress).

## 10. Forks, surfaced

1. **Where it sits in host capabilities' plan (§7):** decided 2026-11-04, Tier 1, step 1.9,
   track D beside 1.6 (messaging), sharing its engine; the room sample needs nothing else from
   Tier 1. Text frames first, binary frames once 1.1 lands.
2. **Subprotocols** (`Sec-WebSocket-Protocol`, which GraphQL over WebSocket and some sync
   libraries use): open's answer names one from the request's list. *Lean:* in the first slice
   if a target library needs it, otherwise the second.
3. **Close with a reason from the flow:** `respond close(4001, "kicked")` from a message run.
   *Lean:* with targeted packs (§11, slice 2).
4. **Rate limits per caller instead of per connection,** since a client can open many.
   *Lean:* `max_websockets` bounds that for now.

## 11. Samples and slices

1. **Text frames:** the statement, lowering, `read_server`, the item, the data plane,
   open/message/close, replies, subscriptions, limits, the log and the connection's record; the
   **`room`** sample (above, with a page at `GET /`, a `js` holon whose script opens the
   connection). The e2e test opens two connections from the browser, sends from one, sees it on
   the other, and closes one to see `left`.
2. **Targeted packs:** `room.send(v, to: conn)`, a pack only the subscriber on that connection
   gets. Direct messages, and answering one connection from another's run or from a clock.
3. **Binary frames**, once `Bytes` lands (§8).

## 12. Refused, with the reason

| Refused | Why |
|---|---|
| A connection the flow writes to directly (`conn.send(x)` from anywhere) | Anything sent must be a recorded effect. A reply (this connection) and a pack (subscribers, or one connection in slice 2) are those effects. A connection handle in a value would also outlive its run. |
| Per-connection memory in the engine | Hidden state a replay can't see. State keyed by `conn` is recorded. |
| Running one connection's messages out of order | A conversation must replay in the order it happened. |
| Messages answered by the host without a run (an echo) | Every message in is a run, so the log holds the whole conversation, except where a person approved a block `unrecorded`, and then the tally counts it. |
| `socket` as the statement's name | Taken by host capabilities §5.9's bound sockets. |

## 13. Invariant check

- **I2 purity:** runs are pure as any route's; the connection is the host's. ✓
- **I7 the effect boundary:** replies and packs are effects the host performs after the run;
  nothing in a run touches the connection. ✓
- **I8 derived labels:** `WebSocket(path)` is read from the server's children, not declared. ✓
- **The record:** every message in is a run, recorded at its block's level (request-path C5);
  everything out is a reply or pack of such a run, or the connection's own record; one
  connection's runs are in order. ✓
- **Request-path I-g and I-h:** a block that sends or writes is `strict`; `unrecorded` is a
  person's approval. ✓
