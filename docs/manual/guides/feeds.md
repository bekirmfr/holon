# Feeds

A flow can run on each event a feed sends: a market data provider's prices, a service's status
changes, anything that streams **server-sent events**. The flow says
`import prices from "feed:prices"` and `on.feed(prices, (e) => …)`; an admin decides that
`prices` is `https://stream.example.com/v1/prices?key=…`. The host holds the connection, keeps
where the feed is, and opens it again when it is lost. The flow never sees the URL, its key or the
headers the host sends.

## Binding a feed

An admin binds a feed from the editor's **admin** page (**resources**, **feeds**), or on the
server's machine. The feed's URL, with any key in it, is kept where a secret's value is: one of
the server's environment variables named `HOLON_SECRET_…` (or a line of its `--dotenv` file), or a
field in Vault. So is each header's value.

```sh
export HOLON_SECRET_PRICES_URL='https://stream.example.com/v1/prices?key=…'
export HOLON_SECRET_PRICES_AUTH='Bearer …'
holon serve
holon feed add prices --url env:HOLON_SECRET_PRICES_URL --header Authorization=env:HOLON_SECRET_PRICES_AUTH
holon feed list
holon feed remove prices
```

| Option | |
|---|---|
| `--url SOURCE` | where the server reads the feed's URL: `env:HOLON_SECRET_…` or `vault:<mount>/<path>#<field>` |
| `--header NAME=SOURCE` | a header each request sends, its value read from a source; again for another |
| `--buffer N` | how many events may wait for their run before the connection waits too: 1000 unless given, 100,000 at most |
| `--idle T` | how long the feed may say nothing before the connection is opened again: `90s` unless given, `5s` at least |
| `--for SLUG` | for that deployment only |

- The URL is `https://…`; plain `http://` only to this machine. A login in it
  (`https://user:password@…`) is sent as Basic authentication. Certificates are checked against
  the public CAs.
- The server reads the feed when it is bound: a binding whose request doesn't answer 200 with
  `text/event-stream` is refused, saying what it answered. A redirect isn't followed, since the
  headers it would carry are the binding's.
- The URL and the headers' values are never stored, shown or logged: the binding keeps
  `env:HOLON_SECRET_PRICES_URL`, and where the URL points without its login or query
  (`https://stream.example.com/v1/prices`), which is what the admin page and `holon feed list`
  show. Each value is read from its source each time a connection opens, so a rotated key is
  used at the next one.
- Several deployments may listen to one feed: each has its own connection and its own place in it.
- Removing a binding closes its connections.

## Listening

```ts
import { flow, on, route } from "holon"
import latest from "collection:latest"
import prices from "feed:prices"
import { now } from "holon:clock"

flow("price-feed", { port: 8187 })

// Each event, {"symbol": "ACME", "price": 10.5}: kept by its symbol.
on.feed(prices, (e) => {
  latest[e.data.symbol] = { price: e.data.price, at: now }
})

route.get("/prices", () => latest.list())
```

The handler runs once for each event, one at a time, in the order they came. `e` is the event:

| Field | |
|---|---|
| `data` | its data: a value when it is JSON, its text when it isn't (several `data:` lines joined with a newline) |
| `event` | its name (`event:`), `"message"` when it gives none |
| `id` | its id (`id:`), only when it has one: a write built from `e.id` waits for nothing when it hasn't |

- One handler a feed. It answers nobody, so it has no `respond`.
- What it may do: "run on each event from the feed "prices"", the grant `FeedEvent("prices")`.
  A deploy is refused while the feed isn't bound.
- The `price-feed` sample is this flow with a page that shows each price as it comes, through a
  channel.

## Where it is

The host keeps, beside the deployment's log, the id of the last event run on and the ids of the
last 1,000:

- **A connection lost** (the feed ended it, the network did, or it said nothing for `--idle`) is
  opened again after a second, sending that id as `Last-Event-ID`, so a feed that keeps its events
  sends what came after it, or after as long as the feed asks (`retry:`), a minute at most. A
  connection that can't be opened is tried again after twice as long each time, up to a minute.
- **An event sent again,** by its id, is passed over, and the timeline says so. An event with no
  id can't be told from another, so it is run on.
- **A new listener** starts at what the feed sends from now. A feed that sends its history to a
  connection without `Last-Event-ID` has it run on, since the host can't tell history from news.
- **A new version** of the deployment, or a binding changed, opens a new connection from where
  the old one was; a binding pointed somewhere else starts anew.
- **Events faster than their runs** wait, up to `--buffer`; past that the host stops reading, and
  the feed's connection waits too, until one has run.
- **An event past 1 MiB** is passed over, the timeline says so, and the rest is read on.

## Its state

The server panel lists each feed handler with what its connection is doing: `connected, at 41`
(the last event run on), or `reconnecting: <why>`, and how many events wait. The deployment's
timeline says when it starts listening, when the feed can't be opened again (once, until it is
read again), and each event it passed over. A run's title is `event from prices: message (on feed prices)`.

The same is in `GET /deployments/<slug>/feeds`:

```json
{"feeds": [{"feed": "prices", "label": "on feed prices", "status": "connected", "since": 1794202012926,
            "address": "https://stream.example.com/v1/prices", "position": "41", "waiting": 0}]}
```

`status` is `connected`, `reconnecting` (with `error`) or `closed` (a deployment stopped).

## What the run log keeps

Each event as it came, `FeedEvent("prices")`: its data, name and id. `holon verify` runs the
handler again on it.
