// Stripe to Slack: a verified webhook, passed on.
//
// Takes Stripe's webhook, checks its signature with the endpoint's signing secret, and posts the
// event's type to a Slack channel through an incoming webhook. A request that isn't signed by
// Stripe (or is too old) gets nothing sent, and a 400.
//
// Each half is a function, one node on the canvas that opens to its steps: fromStripe checks the
// signature, and toSlack posts the line. toSlack sends a request, so it is async and awaits it, as
// a route does.
//
// Its approval says where each secret goes: "read the secret slack_webhook_path, and send it to
// https://hooks.slack.com". The Stripe secret only checks a signature, so it is sent nowhere.
//
// Before it works, set its two secrets in the flow's Settings:
//   stripe_webhook_secret   the endpoint's signing secret from Stripe (whsec_...)
//   slack_webhook_path      your Slack incoming webhook's path (/services/...)
//
// Try it: deploy it, then with the Stripe CLI
//   stripe listen --forward-to 127.0.0.1:8103/stripe
//   stripe trigger payment_intent.succeeded
// and "Stripe: payment_intent.succeeded" shows in the channel.
import { flow, route, status, stripe } from "holon"
import slack_webhook_path from "secret:slack_webhook_path"
import stripe_webhook_secret from "secret:stripe_webhook_secret"
import hooks from "host:hooks.slack.com"
import { now } from "holon:clock"

flow("stripe-to-slack", { port: 8103 })

// Stripe: the event in the request, if Stripe signed it.
function fromStripe(req) {
  const verified = stripe.verify(req, stripe_webhook_secret, now)
  return verified.event
}

// Slack: post the event's type to the channel's incoming webhook, and give back Slack's status.
async function toSlack(event) {
  const sent = await hooks.post(slack_webhook_path, {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text: `Stripe: ${event.type}` }),
  })
  return sent.status
}

// Verify, post to Slack, and answer with Slack's status (400 when the signature fails).
route.post("/stripe", async (req) => {
  const event = fromStripe(req)
  const posted = await toSlack(event)
  return posted ?? status(400)
})
