# WebSockets: a connection whose every message is a run (L4)

**Status:** design, before any code. Written 2026-11-04, its claims checked against the code at
`450e3de` and cited by file and line there.

A flow declares a **socket**: a path a browser opens a WebSocket on. The connection is the
host's to hold. What the flow does happens in runs, as it does for requests:

- **open** is a run when a browser connects. It decides whether to accept, and what the
  connection hears (channel subscriptions).
- **message** is a run per message the browser sends. Its `respond` goes back to that connection
  only.
- **close** is a run when the connection ends, whichever side ends it.

Nothing goes out on a socket except a run's reply or a pack on a channel the connection
subscribed to. Both are already recorded effects of recorded runs. So a connection's whole
conversation is in the log, and every run in it replays as a request's does.

```
flow room
port 8132
channel room
collection people

socket /room/:name {
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
  receive requests
  read items of its collection "people"
  put and delete items of its collection "people"
  send packs on its channel "room" to whoever subscribes
  let callers subscribe to its channel "room"
```

---

## 1. What is already there

- **Channels** (`docs/L4s/channels-L4.md`, built) are the push half of this: a flow sends packs
  from any run, the host delivers them to subscribers after the run commits, keeps the last
  `channel_keep`, drops a slow subscriber, and caps subscribers per deployment
  (`channels.rs` 39–50, 164). Today the only subscriber is a server-sent-events response.
- **Routes** run once per request (`listeners.rs`, `serve_data`) under the deployment's grant,
  with claims ordering runs that touch the same keys. Every request is logged (208).
- **The data plane** is hyper http1 on tokio (`dataplane.rs`), one connection served by
  `serve_connection` (134) without upgrades. hyper 1 supports upgrades
  (`.with_upgrades()`, `hyper::upgrade::on`). There is no WebSocket codec in the tree
  (`Cargo.lock` has no `tungstenite`). `sha1` and `base64`, which the handshake needs, are
  already there.
- **Browsers' `WebSocket`** can't set headers. A page authenticates with a cookie or a token in
  the query string, both of which the opening request carries.

## 2. The connection and its runs

- **The opening request** is a GET with `Upgrade: websocket`. It matches the socket's path like
  a route (a socket's path is a route's, `:params` and `*rest` included), and runs **open** with
  the request record plus `conn`, the connection's id (random, chosen by the host, so it is in
  the record and the log).
- **Open's answer decides the handshake.** A subscription, or a list of them, accepts and
  subscribes. A record with `status` 4xx or 5xx refuses the upgrade with that status. Absent
  (`missing:`) is 403. Any other value accepts and is sent as the first message. Accepting is
  101, sent once open's run has committed, so its writes land before the first message is read.
- **Each text frame** runs **message** with `{conn, params, query, text}`. `json` is
  `parse_json(text)`, lazily, as `req.json` is. A message's `respond` goes to that connection
  only; no `respond` sends nothing.
- **Messages on one connection run one at a time, in order.** The host queues them per
  connection, up to `socket_queue` (32). A full queue closes the connection with 1013 ("try again
  later"). Messages on different connections run concurrently, as requests do, and claims order
  them where they touch the same keys.
- **Packs** on a subscribed channel go out as text frames, each a pack's JSON. They interleave
  with replies in the order the host sends them; a pack goes out after its run commits, as for
  server-sent events.
- **close** runs once, after the last message's run, with `{conn, params, query, code, reason}`.
  It answers nobody.
- **Pings** are the host's: a ping every 15 s, and a connection that doesn't pong within 30 s is
  closed (1001) and its close run runs. Pings aren't runs.
- **Pausing or stopping** a deployment closes its sockets with 1001, and each runs close.
  Redeploying doesn't close them: the next message runs the new version, as the next request
  would.

## 3. The language and what it lowers to

`socket PATH { open (r) {…} message (m) {…} close (c) {…} }`. `open` and `message` are required,
`close` is optional. Each block is a route's block: statements, `respond` (not in `close`), its
own names.

Each lowers to an endpoint-shaped composite with frozen params `declare = "socket"`, `path`, and
`on` (`open`, `message` or `close`), as a route's carries `declare = "endpoint"`, `method` and
`path` (`declare.rs` 1–20). `read_server` collects them into `Server.sockets: Vec<Socket>`, by
path, each with its three handlers' hashes. A socket's path clashes with a GET route's of the same
shape: one path is either a socket or a page.

### Consent

- Opening, messaging and closing are `NetworkIngress`, "receive requests", as routes are.
  Subscribing is `ChannelRead`, sending `ChannelSend`, as now.
- **`max_sockets`** (per deployment, default 100) and **`max_message_bytes`** (default 64 KiB)
  are limits a deploy asks for above their defaults, up to the host's ceilings, as
  `run_budget_ms` is. `messages_per_second` per connection (default 10, bursts of 20) too: past
  it the connection is closed with 1008.

No new capability. A socket adds nothing a flow couldn't already do with POST routes and a
server-sent-events subscription; it holds both on one connection. What the flow may do is the same
list, and the route list shows the socket.

## 4. The log, replay and verify

- **Every run is a log entry,** `kind` `open`, `message` or `close`, `route` `WebSocket
  /room/:name`, with `conn` in its request record. `holon log --conn <id>` reads one connection's
  conversation back, in order.
- **A reply** is the entry's `response_body`, as a request's is. A pack is in its sending run's
  `performed`, as now.
- **Replay and verify** are a request's: the record, the state as the run began, the clock, the
  answers. A connection's order is the order of its entries.
- **Volume** is the price: a chat at ten messages a second writes ten entries a second.

## 5. Limits that stay

- **Latency per message is a run's:** a commit and a log line, milliseconds. Fine for chat,
  presence, notifications, collaborative edits at typing speed. Not for 60 Hz game state or
  cursor streams: those want the browser to batch.
- **No memory per connection except state.** What a connection is (who signed in, which room) is
  kept in a collection or state, keyed by `conn`, and is recorded. There is no hidden
  per-connection variable, by design.
- **Binary frames** are refused (1003) in the first slice: core values are text. A later slice
  can carry a binary message as a handle into a folder, by hash, as uploads are.
- **One machine:** a connection lives on the machine it opened on, like everything else in Holon
  today (README, Status).

## 6. The data plane

- `serve_connection(…).with_upgrades()` (`dataplane.rs` 134). The answer to an upgrade request
  waits for open's run and its commit, then answers 101 and hands the upgraded IO to a task that
  frames it with `tokio-tungstenite` (a new dependency).
- Per connection: a reader (frames → the per-connection queue → a run on the blocking pool, one at
  a time); a writer (replies and packs → frames); a ping timer. A channel subscription is the
  existing `Subscription`, read into the writer.
- Sockets count toward `max_connections` (`dataplane.rs` 48) and `max_subscribers`.

## 7. Samples and slices

1. **Text sockets:** the statement, lowering, `read_server`, the data plane, open/message/close,
   replies, subscriptions, limits, the log; the **`room`** sample (above, with a page at `GET /`:
   a `js` holon whose script opens the socket). The e2e test opens two sockets from the browser,
   sends from one, sees it on the other, and closes one to see `left`.
2. **Targeted packs:** `room.send(v, to: conn)`, a pack only the subscriber on that connection
   gets. Direct messages, and answering one connection from another's run or from a clock.
3. **Binary messages** as handles into a folder.

## 8. Refused, with the reason

| Refused | Why |
|---|---|
| A socket the flow writes to directly (`conn.send(x)` from anywhere) | Anything sent would have to be a recorded effect anyway. A reply (this connection) and a pack (subscribers, or one connection in slice 2) are those effects. A connection handle in a value would also outlive its run. |
| Per-connection memory in the engine | Hidden state a replay can't see. State keyed by `conn` is recorded. |
| A run per frame without order | A conversation must replay in the order it happened. One connection's messages run in order. |
| Messages without a run (an echo the host does) | Every message in is a run, so the log holds the whole conversation. |

## 9. Forks, surfaced

- **Subprotocols** (`Sec-WebSocket-Protocol`, which GraphQL-over-WebSocket and some sync
  libraries use): open's answer could name one from the request's list. Add when a sample needs
  it.
- **Close with a reason from the flow,** `respond close(4001, "kicked")` from a message run.
  Small, and likely wanted with slice 2.
- **Rate limits per caller instead of per connection,** since a client can open many connections.
  `max_sockets` bounds that for now.

## 10. Invariant check

- **I2 purity:** runs are pure as any route's; the connection is the host's. ✓
- **I7 the effect boundary:** replies and packs are effects the host performs after the run;
  nothing in a run touches the socket. ✓
- **I8 derived labels:** the socket is read from the server's children; its permissions are the
  ones its leaves already ask. ✓
- **The record:** every message in is a run; everything out is a recorded run's reply or pack;
  one connection's runs are in order. ✓
