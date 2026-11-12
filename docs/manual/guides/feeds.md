# Feeds

A flow can run on each event a feed sends: a market data provider's prices, a service's status
changes, anything that streams **server-sent events** or speaks over a **WebSocket**, or the rows a
**Postgres** database's trigger sends on a channel. The flow says `import prices from
"feed:prices"` and `on.feed(prices, (e) => …)`; an admin decides that `prices` is
`https://stream.example.com/v1/prices?key=…`, `wss://ws-feed.example.com`, or a channel of a bound
database ([below](#a-postgres-channel)). The host holds the connection, keeps where the feed is,
and opens it again when it is lost. The flow never sees the URL, its key or the headers the host
sends.

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
| `--buffer N` | how many events may wait for their run: 1000 unless given, 100,000 at most |
| `--on-full WHAT` | what comes while that many wait: `wait` (the connection waits too, unless given), `skip` (it is dropped) or `latest` (it replaces the waiting event of its `--key`): [below](#faster-than-their-runs) |
| `--key FIELD` | `latest`'s and `--sequence`'s: the field of an event whose value one event waits by, or is numbered on its own by (`product_id`, or a dotted path, `data.s`) |
| `--sequence FIELD` | the field of a JSON event that numbers it, one after another (`trade_id`), each `--key` on its own: each event says how many numbers were skipped before it ([below](#a-feed-that-numbers-its-messages)) |
| `--idle T` | how long the feed may say nothing before the connection is opened again: `90s` unless given, `5s` at least |
| `--for SLUG` | for that deployment only |

A WebSocket's binding takes these too ([below](#a-websocket)):

| Option | |
|---|---|
| `--subscribe MESSAGE` | a message sent each time the connection opens, as written, or read from a source (`env:…`) for one with a key in it; again for another, sent in order |
| `--heartbeat MESSAGE` | a message sent every `--every T` (`30s` unless given, `1s` to `1h`) while it is open, for a feed that drops a client that says nothing |
| `--id FIELD[,FIELD]` | the fields of a JSON message that are its id, joined with `/`: `--id product_id,sequence` gives `BTC-USD/41` |
| `--only FIELD=VALUE` | run only the messages whose field has that value; again for another value or field (each field named must match) |
| `--resume WHERE` | for a feed that replays: where it takes the id of the message the host is at when a connection opens again, `query:cursor`, `header:NAME` or `message:MESSAGE` ([below](#a-feed-that-replays)) |

A Postgres channel's binding names its database and channel in place of `--url`
([below](#a-postgres-channel)):

| Option | |
|---|---|
| `--database NAME` | the database it is on, as bound (`holon database add`): the one the listening deployment sees by that name, a Postgres |
| `--channel CHANNEL` | the channel listened on (`LISTEN`): letters, digits and `_`, up to 63 bytes |

- The URL is `https://…` for server-sent events, `wss://…` for a WebSocket; plain `http://` and
  `ws://` only to this machine. A login in it (`https://user:password@…`) is sent as Basic
  authentication. Certificates are checked against the public CAs.
- The server reads the feed when it is bound: a binding whose request doesn't answer 200 with
  `text/event-stream`, or a WebSocket's 101, is refused, saying what it answered. A redirect isn't
  followed, since the headers it would carry are the binding's.
- `--subscribe`, `--heartbeat`, `--id`, `--only` and `--resume` are a WebSocket's (`--id` and
  `--only` a channel's too): an SSE binding that gives one is refused, since an SSE event has its
  own id and resumes with `Last-Event-ID`.
  `--on-full latest` without `--key` is refused, and so is `--key` with neither `latest` nor
  `--sequence`.
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
| `data` | its data: a value when it is JSON, its text when it isn't (several `data:` lines joined with a newline); a binary WebSocket message's bytes |
| `event` | its name (`event:`), `"message"` when it gives none (a WebSocket's always); a Postgres channel's name |
| `id` | its id (`id:`, or a WebSocket's `--id` fields), only when it has one: a write built from `e.id` waits for nothing when it hasn't |
| `missed` | how many events the full buffer dropped before it (`--on-full skip`): 0 unless some were |
| `replaced` | how many waiting events of its key it replaced, theirs counted (`--on-full latest`): 0 unless it did |
| `gap` | how many numbers of its key the feed skipped before it (`--sequence`): what came while the host wasn't reading. 0 unless some were |

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
- **A handler that isn't strict** (`{ log: "lazy" }`) moves where it is once its record is on
  disk, at the next sync (a second at most): until then the connection, and a new version's, go on
  from where it is in memory. After a crash, the events of its last second run again, and
  `holon check` says so: `on feed prices is lazy: after a crash, the events of its last second may
  run again`. An unrecorded handler's runs move it at their end.
- **An event past 1 MiB** is passed over, the timeline says so, and the rest is read on.

## Faster than their runs

Events that come faster than their runs wait, up to `--buffer`. What comes while that many wait
is the binding's `--on-full`:

- **`wait`,** unless given: the host stops reading, and the feed's connection waits too, until one
  has run. Nothing is lost, and the feed sees a slow reader.
- **`skip`:** it is dropped, and the next event that finds room says how many were dropped before
  it: `e.missed`. A dropped event's id isn't kept, since it never ran.
- **`latest --key product_id`:** an event replaces the one of its key that waits, in that one's
  place, full or not, and its `e.replaced` is how many it replaced, theirs counted. So each key
  waits once, with its newest value: a run never acts on a price that a newer one has made stale.
  An event without the key waits as it came, and a key that finds the buffer full of others waits
  for room, as with `wait`.

`e.missed`, `e.replaced` and `e.gap` are on every event, 0 unless the binding drops, replaces or
numbers, so a flow reads them whatever its binding says.

## A WebSocket

```sh
export HOLON_SECRET_TICKER_URL='wss://ws-feed.exchange.coinbase.com'
holon feed add ticker --url env:HOLON_SECRET_TICKER_URL \
  --subscribe '{"type":"subscribe","channels":["ticker"],"product_ids":["BTC-USD"]}' \
  --only type=ticker --id product_id,sequence --on-full latest --key product_id
```

- **Each message** is an event: a text message's data is a value when it is JSON, else its text;
  a binary message's data is its bytes. A message past 1 MiB is passed over, as an SSE event is;
  past 16 MiB, the connection ends and is opened again.
- **What it is sent:** each `--subscribe` message, in order, each time the connection opens, and
  the `--heartbeat` every `--every`. Neither starts a run. The feed's pings are answered.
- **Which messages run:** with `--only`, those whose field has a value given. The rest (a
  subscription's answer, a heartbeat's) are passed over without a run, counted in the feed's
  state, not on the timeline.
- **Where it is:** with `--id`, a message's id is its fields joined with `/`, and the last 1,000
  run on pass over a message sent again: the snapshot a feed sends after it subscribes again, say.
  A WebSocket has no `Last-Event-ID`: a connection opened again subscribes again and is read from
  what the feed sends then, unless the binding says where the feed takes the id
  ([below](#a-feed-that-replays)).
- The `trading-bot` sample binds Coinbase Exchange's public ticker this way, keeps each product's
  newest price, and places an order through a strict action when a price comes under a level.

## A feed that replays

Some WebSocket feeds send again what came after a message, when a connection asks from it:
Bluesky's Jetstream takes `?cursor=<seq>`, others a header or a field of the subscription. The
binding says where, and the host opens a lost connection again from where it is, as SSE does with
`Last-Event-ID`:

```sh
export HOLON_SECRET_POSTS_URL='wss://jetstream.us-east.bsky.network/xrpc/network.bsky.jetstream.subscribeEvents?collections=app.bsky.feed.post'
holon feed add posts --url env:HOLON_SECRET_POSTS_URL --id payload.seq --resume query:cursor \
  --only payload.operation=create
```

| `--resume` | where the id goes |
|---|---|
| `query:NAME` | `NAME=<id>` in the URL's query, in place of a parameter of that name in it |
| `header:NAME` | a header `NAME: <id>` |
| `message:MESSAGE` | sent in place of the `--subscribe` messages, `{position}` in it the id: `message:{"op":"subscribe","from":{position}}`; again for another, in order; as written, or read from a source (`env:…`) |

- **The id sent** is the one SSE sends: on a reconnect, the last message read, since those waiting
  still run; after a restart or for a new version, the last one run on, a lazy handler's once its
  record is synced. A new listener sends none, and reads from what the feed sends then.
- **It takes an `--id` of one field:** a cursor is one value.
- **The message at the cursor,** which a feed that replays from it sends again, as Jetstream
  does, is passed over by its id, on the timeline: `passed over an event of posts it ran already
  (26984429105)`. A feed that sends at least once may send others twice too: a handler that writes
  by the message's own key (a post's address) writes the same item.
- **An id older than what the feed keeps** is the feed's to answer: what it lost meanwhile, the
  host can't ask for again.
- The `bluesky-mentions` sample binds Jetstream this way and keeps each post that mentions a word.

## A feed that numbers its messages

Some feeds number what they send, one after another, and never send it again: an exchange's
trades, each product's on its own. What came while the host wasn't reading (a connection lost,
the server stopped) is the feed's to forget, but the next message says how much: its number is
past the last one read by more than one. The binding names the field, and the key each is
numbered by:

```sh
export HOLON_SECRET_TRADES_URL='wss://ws-feed.exchange.coinbase.com'
holon feed add trades --url env:HOLON_SECRET_TRADES_URL \
  --subscribe '{"type":"subscribe","channels":["matches"],"product_ids":["BTC-USD","ETH-USD"]}' \
  --only type=match --only type=last_match --id product_id,trade_id --sequence trade_id --key product_id
```

- **`e.gap`** is how many numbers of its key were skipped before it: 2 for 41 after 38. It is 0
  for the first of a key, and for a message whose field isn't a whole number. The flow fetches
  what was skipped, from the feed's own API, or counts it.
- **Across a reconnect or a restart:** each key's last number is kept with where the feed is (the
  last 10,000 keys, the least recent forgotten), so the first message after the server stopped
  says what came meanwhile.
- **A number not above the last:** the first of its key on a new connection begins the count
  anew from it, for a feed that numbers each connection on its own, or replays; a later one is
  one sent again, its gap 0.
- **None goes unsaid:** a message `--only` passes over still counts, and its gap goes to the next
  of its key that runs; so does the gap of one the full buffer drops (`skip`), and an event that
  replaces another (`latest`) adds the other's.
- **After a restart,** the numbers kept are the last runs': the messages of a key that `--only`
  passed over, or `skip` dropped, since its last run count in its next gap.
- It is any feed's: an SSE event's numbers too, when its data is JSON.
- The `trade-tape` sample binds Coinbase Exchange's trades this way, adds each trade to its
  product's volume, and fetches the trades a gap skipped from the exchange's REST API.

## A Postgres channel

A Postgres database sends notifications on a channel (`NOTIFY`, `pg_notify`), each when the
transaction that sent it commits: a trigger on a table can send each row as it is written. A feed
can be a channel of a database bound to the server ([Databases](databases.md)): the host listens
on it (`LISTEN`), and each notification is an event.

```sh
export HOLON_SECRET_LEDGER_URL='postgres://holon:…@db.example.com:5432/ledger'
holon database add ledger env:HOLON_SECRET_LEDGER_URL --read-only --schema-file ledger.sql
holon feed add entries --database ledger --channel ledger_entries
```

where `ledger.sql` makes the table and the trigger that sends each entry:

```sql
create table if not exists entries (id bigint generated always as identity primary key,
  account text not null, seq integer not null, amount numeric(18,2) not null, memo text,
  unique (account, seq));
create or replace function entries_notify() returns trigger language plpgsql as $$ begin
  perform pg_notify('ledger_entries', json_build_object('id', new.id, 'account',
    new.account, 'seq', new.seq, 'amount', new.amount::text, 'memo', new.memo)::text);
  return null; end $$;
create or replace trigger entries_notify after insert on entries for each row
  execute function entries_notify();
```

- **Its data** is the notification's payload: a value when it is JSON, else its text. `e.event`
  is the channel. `--id`, `--only`, `--sequence` and `--key` read its fields as a WebSocket's;
  `--url`, `--header`, `--subscribe`, `--heartbeat` and `--resume` are refused with a channel.
- **Bound once it listens:** the server connects to the database, listens on the channel, and
  closes it; a database that isn't bound, or isn't a Postgres, is refused. The binding keeps the
  database's name, and its connection is read as each listening deployment sees the name, each
  time a connection opens: a deployment with a `ledger` of its own listens there. `holon feed
  list` shows `postgres://db.example.com:5432/ledger, channel ledger_entries`.
- **A session of its own:** each listening deployment has its own connection, not one of the
  database's pool, since a `LISTEN` belongs to the session that said it. Several deployments may
  listen to one channel, and each is sent every notification. It connects as the database's pool
  does: with TLS when the server offers it.
- **In the order committed:** Postgres sends a transaction's notifications when it commits, in
  the order transactions commit; a transaction rolled back sends none. Two the same (channel and
  payload) in one transaction are sent once. A payload is 8,000 bytes at most: a trigger sends a
  row's key and what a run needs, and the run reads the rest from the table.
- **Quiet:** after `--idle` with no notification, the host asks the database whether the
  connection is still there, and opens it again only when it doesn't answer.
- **What is sent while it isn't listening is lost:** Postgres keeps no notification for a session
  that isn't listening. A connection lost is opened again after a second (twice as long each time
  it can't be, to a minute), and a new version or a restart opens a new one; each time it is
  opened again, the timeline says `listens to the feed entries again: what was sent meanwhile is
  lost`. What was written is still in the table, so the flow reads it there: the rows after the
  last it has seen, by a number the table keeps (`--sequence` makes it `e.gap`).
- The `ledger-watch` sample keeps each account's balance from its ledger's entries this way, and
  reads from the ledger the entries a lost connection didn't hear.

## Its state

The editor's Try a request lists each feed handler with what its connection is doing: `connected, at 41`
(the last event run on), or `reconnecting: <why>`, how many events wait, and, since its
connection opened, how many newer ones replaced (`latest`), the full buffer dropped (`skip`) and
`--only` passed over, and how many numbers the feed skipped (`--sequence`): `connected, at
BTC-USD/41, 3 waiting, 120 replaced`, `connected, at BTC-USD/1117440338, 49 in gaps`. The deployment's
timeline says when it starts listening, when the feed can't be opened again (once, until it is
read again), and each event it passed over. A run's title is `event from prices: message (on feed prices)`.

The same is in `GET /deployments/<slug>/feeds`:

```json
{"feeds": [{"feed": "prices", "label": "on feed prices", "status": "connected", "since": 1794202012926,
            "address": "https://stream.example.com/v1/prices", "position": "41", "waiting": 0}]}
```

`replaced`, `missed`, `passed_over` and `gaps` are there too once they aren't 0.

`status` is `connected`, `reconnecting` (with `error`) or `closed` (a deployment stopped).

## What the run log keeps

Each event as it came, `FeedEvent("prices")`: its data (a binary message's bytes as any bytes
value is), name, id, `missed`, `replaced` and `gap`. `holon verify` runs the handler again on it.
