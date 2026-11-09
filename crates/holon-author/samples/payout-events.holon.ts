// Payout events: a provider's events from a queue, each a run; a failed payout's retry to another.
//
// A payment provider sends its payout events as webhooks, which a forwarder of its own puts on an
// SQS queue. Each message the queue hands out starts one run of the handler, one at a time, in the
// order received: m is the message, its data a value since it is JSON. The host deletes it once its
// run's record is on disk, so a crash runs it again rather than losing it, and a message handed
// out twice (a standard queue may) is passed over. Each payout's status is kept by its id; a failed
// one is sent to a FIFO queue of retries, its group the account, so one account's retries keep
// their order. It goes once the run is over and its writes have landed. The flow never sees either
// queue's URL or its key.
//
// Before it works, an admin binds both queues, each URL with its key in a secret source:
//   HOLON_SECRET_EVENTS_URL=https://AKIA…:<secret>@sqs.eu-west-1.amazonaws.com/123456789012/payout-events
//   HOLON_SECRET_RETRIES_URL=https://AKIA…:<secret>@sqs.eu-west-1.amazonaws.com/123456789012/payout-retries.fifo
//   holon queue add payout_events --url env:HOLON_SECRET_EVENTS_URL
//   holon queue add payout_retries --url env:HOLON_SECRET_RETRIES_URL
//
// Try it: deploy it, and send the queue an event
//   aws sqs send-message --queue-url https://sqs.eu-west-1.amazonaws.com/123456789012/payout-events \
//     --message-body '{"type":"payout.failed","payout":{"id":"po_1","amount":"120.00","currency":"EUR","account":"acct_1"}}'
//   curl http://127.0.0.1:8194/payouts      {"po_1": {"status": "failed", …}}
import { flow, on, route } from "holon"
import payouts from "collection:payouts"
import payout_events from "queue:payout_events"
import payout_retries from "queue:payout_retries"

flow("payout-events", { port: 8194 })

// Each event, {"type": "payout.paid" | "payout.failed", "payout": {id, amount, currency, account}}:
// the payout's status kept by its id, a failed one sent to be tried again.
on.queue(payout_events, (m) => {
  const payout = m.data.payout
  payouts[payout.id] = {
    amount: payout.amount,
    currency: payout.currency,
    account: payout.account,
    status: m.data.type.replace("payout.", ""),
    event: m.id,
    at: m.sent,
  }
  if (m.data.type === "payout.failed") {
    payout_retries.send({ data: payout, group: payout.account })
  }
})

// Each payout's status, by its id.
route.get("/payouts", () => payouts.list())
