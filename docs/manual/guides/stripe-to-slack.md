# Stripe to Slack

This guide runs a real webhook end to end: Stripe sends an event, a flow checks Stripe's
signature, and posts the event to a Slack channel. It uses Stripe's test mode, so no money moves.
It takes about twenty minutes.

You need:

- a Holon server you can sign in to as an editor or admin ([Getting started](../getting-started.md));
  setting secrets needs an admin;
- a Stripe account (test mode is enough) and the [Stripe CLI](https://docs.stripe.com/stripe-cli),
  signed in with `stripe login`;
- a Slack workspace where you may add an app.

## The flow

```holon
flow stripe-to-slack
port 8103

route POST /stripe (req) {
  verified = stripe.verify(req, secret.stripe_webhook_secret, now)
  sent = http.post("hooks.slack.com", secret.slack_webhook_path,
    headers: { "content-type": "application/json" },
    body: json({ text: "Stripe: {verified.event.type}" }))
  respond sent.status, missing: 400
}
```

- `stripe.verify` checks the `Stripe-Signature` header the way Stripe's own libraries do: an
  HMAC-SHA256 of the timestamp and the raw body under the endpoint's signing secret, compared in
  constant time, with the timestamp no more than five minutes from `now`. Only then does it give
  the event.
- `http.post` sends the message to Slack. The host is written in the text, so it is in the
  permissions; the path of the webhook URL is a secret, because anyone who has it can post to
  your channel.
- `respond sent.status` answers Stripe with Slack's status. When the signature doesn't verify,
  `verified` has no value, so nothing is posted, and the route answers 400.

## 1. Make a Slack incoming webhook

In Slack, create an app for your workspace ("From scratch"), turn on **Incoming Webhooks**, and
**Add New Webhook to Workspace** for the channel you want. Slack gives a URL like:

```
https://hooks.slack.com/services/T0000/B0000/XXXXXXXXXXXXXXXX
```

The flow needs the path part: `/services/T0000/B0000/XXXXXXXXXXXXXXXX`.

## 2. Deploy the flow

In the editor, on the **flows** page, open the **Stripe → Slack** sample (or import the flow above
as a `.holon` file). Press **deploy**. It asks to:

- read the clock
- call https://hooks.slack.com
- receive requests
- read the secret "slack_webhook_path"
- read the secret "stripe_webhook_secret"

and says neither secret is set yet. Press **Approve and deploy**.

## 3. Set the Slack secret

Press **⚙**, and under **secrets**, set `slack_webhook_path` to the path from step 1 (source
**value**). It is sealed on the server and never shown again; in the run log it appears as
`‹secret:slack_webhook_path›`.

## 4. Forward Stripe's events

In a terminal:

```sh
stripe listen --forward-to 127.0.0.1:8103/stripe
```

```
> Ready! Your webhook signing secret is whsec_1234… (^C to quit)
```

Set `stripe_webhook_secret` to that `whsec_…` value, the same way as the Slack one.

## 5. Send an event

In another terminal:

```sh
stripe trigger payment_intent.succeeded
```

Stripe creates a test payment and sends its events. In the `stripe listen` terminal, each
delivery shows `[200]`, and the channel gets a message per event, such as:

```
Stripe: payment_intent.created
Stripe: charge.succeeded
Stripe: payment_intent.succeeded
```

## 6. Look at the runs

Switch the flow's tab to **view**. Each delivery is a run in the **Log**. Pick one:

- the request, with its `stripe-signature` header;
- the clock it read, and the secrets, shown as `‹secret:…›`;
- the call to `hooks.slack.com` and Slack's answer, between rounds;
- what the route answered: `200`.

**✓ replay reproduces the run**: the editor ran the flow again from the log and got the same.

## 7. A failure, and finding it

Send a request Stripe didn't sign:

```sh
curl -i -X POST http://127.0.0.1:8103/stripe -d '{"type":"fake.event"}'
```

```
HTTP/1.1 400 Bad Request
```

In view mode, the newest run answered 400. On the canvas, the verify node has no value, and
everything after it is dimmed: the post to Slack never ran, so nothing reached the channel. The
inspector, on a dimmed node, says why it didn't run.

A wrong Slack path fails differently: the post is sent, Slack answers with an error status, and
the route answers Stripe with that status, so Stripe retries the delivery later. The run's details show Slack's answer. Fix the
secret under **⚙ → secrets**, and the next delivery goes through.

An agent sees the same through `holon mcp`: `runs` lists the deliveries, and `explain` shows a
run as the flow's text with each statement's value.

## 8. Use a real endpoint

`stripe listen` is for testing. For real events, Stripe needs a public HTTPS address:

1. Put the flow's port behind HTTPS: [A public HTTPS address](public-https.md).
2. In Stripe's dashboard, under **Developers → Webhooks**, add an endpoint with that address and
   the path `/stripe`, and choose its events.
3. Stripe shows the endpoint's own signing secret (`whsec_…`): set `stripe_webhook_secret` to it.
   Each endpoint has its own, so the `stripe listen` one no longer verifies.

Every delivery is now a run in the log, kept and checkable: see
[Checking runs](checking-runs.md).
