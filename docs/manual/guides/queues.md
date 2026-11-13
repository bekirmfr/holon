# Queues

A flow can take messages from a queue and send to one: a payment provider's events that a
forwarder puts on a queue, work another service hands out, retries to be done later. Holon speaks
**Amazon SQS**'s API, and so reaches SQS and the services that copy it (ElasticMQ, LocalStack).
The flow says `import payout_events from "queue:payout_events"` and runs
`on.queue(payout_events, (m) => …)` for each message the queue holds, or sends with
`payout_retries.send({ data })`; an admin decides which queue each name is, and the key it is
reached with. The flow never sees the URL or the key.

## Binding a queue

An admin binds a queue from the editor's **admin** page (**resources**, **queues**), or on the
server's machine. The queue's URL, with its access key in it, is kept where a secret's value is:
one of the server's environment variables named `HOLON_SECRET_…` (or a line of its `--dotenv`
file), or a field in Vault.

```sh
export HOLON_SECRET_EVENTS_URL='https://AKIA…:<secret>@sqs.eu-west-1.amazonaws.com/123456789012/payout-events'
holon serve
holon queue add payout_events --url env:HOLON_SECRET_EVENTS_URL
holon queue list
holon queue remove payout_events
```

| Option | |
|---|---|
| `--url SOURCE` | where the server reads the queue's URL: `env:HOLON_SECRET_…` or `vault:<mount>/<path>#<field>` |
| `--visibility T` | how long a message received is kept out of sight while it waits for its run and runs: `30s` to `12h`, `5m` unless given |
| `--keep-failed` | leave a failed run's message in the queue, for it to hand out again and its redrive policy to count, rather than delete it |
| `--for SLUG` | for that deployment only |

- **The URL is the queue's own** (its `QueueUrl`), logged in with an access key: its id and its
  secret, the secret percent-encoded (a secret holds `/` and `+`: `%2F`, `%2B`). The region is
  the host's (`sqs.<region>.amazonaws.com`), or `?region=` for a service that copies the API
  under another name. A temporary key's session token is `?token=`. Plain `http://` only to this
  machine: `http://<id>:<secret>@127.0.0.1:9324/000000000000/payout-events?region=elasticmq`.
- **The server asks the queue when it is bound,** with the key: a wrong key, a wrong region or a
  queue that isn't there is refused, saying what the service answered. It reads whether the
  queue is FIFO and the largest message it takes, and keeps both with the binding.
- **The URL is never stored, shown or logged:** the binding keeps `env:HOLON_SECRET_EVENTS_URL`,
  and where the queue is without its login or query
  (`https://sqs.eu-west-1.amazonaws.com/123456789012/payout-events`), which is what the admin page
  and `holon queue list` show. It is read from its source again when a listener starts, so a
  rotated key is used then.
- **The names are the flow's:** letters, digits and `_`. The queue's own name may be anything SQS
  takes (`payout-events`, `payout-retries.fifo`).
- **One deployment listens to a queue,** as to a mailbox: a message is deleted once, so a second
  listener would take the first one's messages. A deploy that listens to a queue another listens
  to, under whatever name, is refused, and so is one whose queue isn't bound. Any number may send
  to it.
- Removing a binding stops its listener. What the queue holds stays in it.

The key needs these actions on the queue: `sqs:GetQueueAttributes`, `sqs:ReceiveMessage`,
`sqs:DeleteMessage` and `sqs:ChangeMessageVisibility` to listen, `sqs:SendMessage` to send.
Binding asks `GetQueueAttributes` alone, so a key without the rest is bound all the same: a
listener's receive is refused (the editor's Try a request says `unreachable`, with what SQS answered), and
a send is given up on the timeline (`answered 403 to SendMessage: AccessDenied: …`).

## Listening

```ts
import { flow, on, route } from "holon"
import payouts from "collection:payouts"
import payout_events from "queue:payout_events"

flow("payout-events", { port: 8194 })

// Each event, {"type": "payout.paid", "payout": {id, amount, currency, account}}: kept by its id.
on.queue(payout_events, (m) => {
  const payout = m.data.payout
  payouts[payout.id] = { amount: payout.amount, status: m.data.type.replace("payout.", ""), event: m.id }
})

route.get("/payouts", () => payouts.list())
```

`m` is the message:

| Field | |
|---|---|
| `data` | its body: a value when it is JSON, else its text |
| `id` | the queue's id for the message |
| `attributes` | its message attributes whose type is `String` or `Number`, as text |
| `sent` | when it was sent, in seconds since 1970, as `now` is |
| `count` | how many times the queue has handed it out |
| `group` | a FIFO queue's message group |

- One handler a queue. It answers nobody, so it has no `respond`.
- What it may do: "run on each message from the queue "payout_events", and delete it once its run
  is over", the grant `QueueMessage("payout_events")`.
- **One at a time,** in the order received: the host long-polls the queue (up to 10 messages,
  waiting up to 20 s for one), runs those it receives one after another, and asks again once
  they have run. A FIFO queue hands out a group's messages in the order sent.
- **Deleted once its run's record is on disk:** a crash before then runs the message again,
  rather than losing it. A failed run's message is deleted too, its record saying why, unless the
  binding says `--keep-failed`: then the queue hands it out again once it is visible again, and
  its redrive policy moves it aside after as many tries as it says.
- **Kept out of sight while it waits and runs:** every half of `--visibility`, the host keeps
  what it received and hasn't deleted out of sight for as long again, so a long run's message
  isn't handed to another reader meanwhile.
- **Once per message:** the ids of the last 1,000 messages run on are kept beside the
  deployments. A message handed out again (one whose delete a crash cut short, or a standard
  queue's second delivery, which SQS allows) is deleted without a run, and the timeline says so:
  `passed over a message of payout_events it ran already (…)`.
- **When the queue can't be reached,** the host asks again after a second, twice as long each
  time, up to a minute. The timeline says so once, and again when it can be.
- **Stopped** (a new version, the deployment stopped, the binding removed), the messages received
  that haven't run are made visible at once, for the next listener.
- **A handler that isn't strict** (`{ log: "lazy" }`) deletes its message once the sync covering
  its record is done. After a crash, the messages of its last second run again, and `holon check`
  says so. An unrecorded handler's run deletes its message at its end.

## Sending

```ts
import { flow, route } from "holon"
import payout_retries from "queue:payout_retries"

flow("retry-later", { port: 8195 })

// A payout to try again, {"id", "amount", "currency", "account"}: sent once the run is over.
route.post("/retries", (req) => {
  payout_retries.send({ data: req.json, group: req.json.account })
  return { queued: req.json.id }
})
```

- **The body** is `data`: text as it is, anything else as JSON. Bytes, or an empty body, fail the
  run, and so does a body past the largest the queue takes (256 KiB unless it says more). A
  message without `data` sends nothing.
- **It goes once the run is over** and its writes have landed: the message is kept on disk with
  the run, before its record is written, and sent once the record is. A run that fails sends
  nothing; a restart sends what a run whose record is on disk had kept. Each queue's messages go
  in the order sent, one queue's not waiting for another's.
- **Tried again** when the queue can't take it yet (no connection, a 5xx, a throttle): after 1 s,
  10 s, a minute, then every 5 minutes, for a day. The timeline says so once, and when it is sent
  again; a refusal (another 4xx) or a day of trying gives it up, on the timeline too.
- **A FIFO queue** keeps a group's messages in the order sent: `group` names it (text, 1 to 128
  characters; another value sends nothing). Without one, the deployment's name is the group, so
  its messages keep their order. Each message's deduplication id is its run's and its place in
  the run, so one tried again after a lost answer is taken once. A standard queue has no such id:
  a retry after a lost answer may send it twice, and its group is not sent.
- **At most 100 sends a run;** past it the run fails, before anything is sent.
- **A message computed from a secret is refused,** as a write of one is.
- What it may do: "send messages to the queue "payout_retries"", the grant
  `QueueSend("payout_retries")`.

## Its state

The editor's Try a request says, for each queue a deployment listens to, what its listener is doing
(`receiving, 4 run, 1 passed over`), and for each it sends to, what its messages came to
(`2 sent, 1 waiting`). `GET /deployments/<slug>/queues` answers the same:

```json
{
  "listening": [{ "queue": "payout_events", "label": "on queue payout_events", "address": "https://sqs.eu-west-1.amazonaws.com/123456789012/payout-events", "status": "receiving", "since": 1794220281022, "waiting": 0, "run": 4, "passed_over": 1 }],
  "sending": [{ "queue": "payout_retries", "address": "https://sqs.eu-west-1.amazonaws.com/123456789012/payout-retries.fifo", "sent": 2, "waiting": 0 }]
}
```

`status` is `receiving` or `unreachable` (with `error`); `kept` counts the failed runs' messages
left for the redrive. What was sent is counted since the server started.

The `payout-events` sample keeps each payout's status from a provider's events on one queue, and
sends a failed payout to a FIFO queue of retries, its group the account.
