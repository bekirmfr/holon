# WebSockets

A page can keep a connection to a flow open and talk over it both ways: a chat, a board people
edit together, a game, a dashboard that sends commands. The flow says
`route.websocket("/room/:name", { open, message, close })`, and each thing that happens to a
connection is a run of its own: its opening, each text the page sends, its end. The host holds
the connections, never a run: a run is over in milliseconds, and what outlives it is in the
flow's state, its collections and its channels. Nothing is bound: a websocket is on the flow's own
port, beside its routes.

## A chat in one flow

The `room` sample (`holon new room.holon.ts --sample room`, or the **Library** in the editor)
without its page:

```ts
import { flow, route } from "holon"
import people from "collection:people"
import room from "channel:room"

flow("room", { port: 8132 })

route.websocket("/room/:name", {
  open: (req) => {
    people[req.conn] = req.params.name
    room.send({ joined: req.params.name })
    return room.subscribe()
  },
  message: (m) => {
    room.send({ from: people[m.conn], text: m.text })
  },
  close: (c) => {
    delete people[c.conn]
    room.send({ left: c.params.name })
  },
})
```

Its page opens `new WebSocket("ws://" + location.host + "/room/" + name)`. `open` keeps who joined
under the connection's id, `req.conn`, tells the room and subscribes the connection to the
channel `room`; `message` says each text to the room under its sender's name; `close` forgets
them and says who left. Deploy it, open http://127.0.0.1:8132/ in two browsers, join under two
names and say something in one: it shows in both.

## The blocks

Each block is a handler of its own, with its own record and its own answer. One block at least,
each written once.

| Block | Runs | Its record | Its answer |
|---|---|---|---|
| `open` | as a page opens a connection | the opening request (`path`, `params`, `query`, `headers`, …) and `conn` | as a route's: `return ch.subscribe()` accepts and subscribes, a `status` of 300 or more refuses (the page sees the opening fail), `status(204)` or effects alone accept and send nothing, anything else accepts and is the first message |
| `message` | for each text the page sends, one at a time, in order | `conn`, `text`, `json` (when it is JSON), `path`, `params`, `query` | a value alone, sent back on the connection (text as it is, anything else as JSON); none sends nothing |
| `close` | once the connection has ended, after its last message has run | `conn`, `code`, `reason`, `path`, `params`, `query` | nobody |

- **`conn`** is the connection's id, 16 hex digits, the same in each of its runs: the key to keep
  what the flow knows of the connection under. It gives nothing to whoever has it.
- **Without `open`,** a connection is accepted as it comes, and sent nothing. **Without
  `message`,** one that sends a message is closed (1003). A message's run that fails sends nothing
  back; its record says why.
- **A path is a page or a connection:** a websocket whose path matches a GET route's is refused,
  as are two websockets alike. A GET on its path that isn't a WebSocket's opening is answered 426.

## Reaching the other connections

A run answers only its own connection. To reach the others, send on a channel that they are
subscribed to, as the room does: `open` returns `room.subscribe()`, and each pack sent on `room`
from then on goes to the connection as a text message, the pack as JSON. A pack goes out once its
run is over, its writes have landed and its record is on disk, so a page never hears of a message
that wasn't kept; a run that fails sends nothing. A connection is subscribed once its `open` has
run, and gets the packs that go out from then on (`subscribe({ from: "kept" })` sends the ones
the channel keeps first). See [the channels in the language
reference](../language/reference.md#the-outside-world).

## What the host holds

The host answers a connection's pings, pings it every 15 seconds, and closes it when it hears
nothing for 30. It holds each within the deployment's limits (in the editor's **Settings**, or the
flow's `limits`; [Permissions](../reference/permissions.md#limits)):

| Limit | Is | Default | Ceiling |
|---|---|---|---|
| `max_websockets` | connections the deployment holds at once | 100 | 10000 |
| `max_message_bytes` | a message's size, in bytes | 65536 (64 KiB) | 1048576 (1 MiB) |
| `messages_per_second` | messages each connection may send a second, in bursts of twice that | 10 | 1000 |

A connection subscribed to a channel is one of its subscribers too (`max_subscribers`). An
opening the host refuses never runs `open`, and is answered with an HTTP status:

| Status | Why |
|---|---|
| 426 | a GET on the path that isn't a WebSocket's opening |
| 403 | a page from another site: its `Origin` must be the flow's own host or one its `browsers` names (a program that sends no `Origin` may connect) |
| 503 | the flow isn't running, or holds `max_websockets` already |

An opening `open` accepts but the host can't hold after all (its channel has all the subscribers
it may) is answered 503 too, and its `close` runs, as a dropped connection's does, so what `open`
kept is let go. A connection the host ends is closed with a code that says why, and its `close`
runs with that code:

| Code | Why |
|---|---|
| 1001 | nothing heard for 30 s; the flow was paused, stopped or removed; or its new version has no websocket on the path |
| 1003 | a binary message, or a message to a websocket without a `message` block |
| 1007 | a text message that isn't UTF-8 |
| 1008 | more than `messages_per_second` |
| 1009 | a message past `max_message_bytes` |
| 1013 | 32 messages waiting for their runs, or too slow to read its channel's packs |

A page that closes its connection gives its own code (1000, 1001 as it goes away), or none
(1005); one that drops without closing is 1006.

**A new version keeps the connections:** each block runs as the version deployed when it runs, so
a connection open across a deploy has its next message run by the new `message`. Pausing or
stopping the deployment closes them all (1001), and their `close` blocks run all the same.
**Some ends run no `close`,** as there is no block left to run: a new version without a websocket
on the path closes the connections there (1001), removing the deployment closes them all (1001),
and when the server itself stops its connections end with it. What `open` kept of those stays: a
flow whose state must not hold connections that are gone can clear what it keeps of them with an
action.

## In the log

Each block's run is a line of the deployment's log, `kind` `websocket`, named for its block:
`WebSocket /room/:name message`. Its request is the block's record. An `open` that accepts is
logged 101, one that refuses with its status; a `message` or `close` has a status only when it
fails. Each is replayed in the editor, run again from its record by `holon verify`, and replayed
on a new version before it goes live, as a route's is.

A block is strict unless it says otherwise: its line is on disk before its answer goes, and before
its packs do. A chat's messages come often and say little worth keeping a second for:
`{ log: { message: "lazy" } }` makes the `message` block's runs answer at once, their lines on disk
within a second (and their packs once they are). `{ log: "lazy" }` does so for every block.

Each websocket is a permission a person approves at deploy, `WebSocket("/room/:name")`, shown as
"hold WebSocket connections on /room/:name, and run on each message", as the channel and the
collection it uses are.
