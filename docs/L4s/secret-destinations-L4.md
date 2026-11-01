# Secret destinations: where a secret goes is shown where a person decides (L4)

**Status:** built, in its reduced form (2026-11-02), on `claude/declared-names-ef7be1`: the
commits "secrets: the approval says where each secret is sent" and "outbound: a redirect
elsewhere doesn't resend the body". The first version of this note proposed a grant item per
secret and host, enforced on every request; §3 says why that was dropped. Claims about the code
are cited by file.

A flow's header declares the secrets it reads (`secret stripe_key`, docs/L4s/declared-names-L4.md),
and the deploy approval lists each one ("read the secret "stripe_key"") beside each host the flow
calls ("call https://api.stripe.com"). Read as two lists, they don't say which secret goes to
which host: a flow approved to read `stripe_key` and to call `api.stripe.com` and
`hooks.slack.com` could send the key to either. This note makes the approval say it:

> **The approval says where each secret goes**, beside the secret: "read the secret "stripe_key",
> and send it to https://api.stripe.com". It is derived from the graph before the flow runs, so
> it is exactly what the flow can do, and a person sees it when they approve.

## 1. Where a secret can go

`holon_core::taint` follows a secret's value through a run's graph before it runs, as
**literal** (the value as it is) or **derived** (computed from it: base64, a slice, a field).
One-way leaves (`sha256`, `hmac_sha256`, `secure_equals`), a Boolean, and a call's answer cut the
chain; it can follow one secret alone (`SecretFed::of_secret`).

| Destination | What happens |
|---|---|
| The run log, exports, the editor | Redacted (literal) or withheld (derived): `secrets.rs`, `taint.rs` |
| State, collections, folders, database writes, channel packs | Refused, literal or derived: `firing.rs` |
| Another flow's action (`calls billing`) | Refused: `calls.rs` |
| An action's answer | Withheld and redacted: `deploy.rs` |
| A route's answer | Only where approved, `SecretAnswer("name")`: `listeners.rs` |
| An outbound HTTP request (path, headers, body) | To the hosts the flow names and a person approved; **the approval says which secret goes to which** (§2) |
| A redirect of such a request, to another host | Without the flow's headers but those that say what the body is (`CARRIED_ELSEWHERE`), and not followed when it would resend a body (§4) |
| A database read's parameters (`d.query("… $1", secret.x)`) | Sent to the database: not shown yet (§6) |

## 2. Where each secret is sent, derived

A flow **sends a secret to a host** when the secret's value, or something computed from it, may
reach an outbound request to that host: its path, headers or body. Core derives it,
`taint::sent_secrets(root, resolver) -> secret → hosts`, by the walk that exists: a request leaf
(any leaf whose capability is `HttpRequest(host)`: `http_request`, `http_upload`, `http_stream`)
fed a tainted input sends each followed secret to its host, and a node the walk doesn't follow
(a `map` over a function that reads one, a module) counts every host it could call. A request's
host is a frozen param, so every pair is known before the flow runs.

It is said beside the secret's item, by one function (`describe::describe_item_sending`):

```
read the secret "slack_webhook_path", and send it to https://hooks.slack.com
read the secret "stripe_webhook_secret"
```

(stripe-to-slack: the Stripe secret only verifies a signature, so it is sent nowhere.) Where:

- the editor's deploy approval, from the deploy preview's `sent` (`deploy.rs`), both under "It
  needs your approval to" and "Already granted";
- `holon check`, `holon deploy` (its "It asks to" and "approved, as you" lists) and the agent
  tools' deploy and save answers (`agent::Plan::sent`);
- the editor's hover on `secret.x`, and on `secret x` in the header: "This flow sends it to
  https://hooks.slack.com".

## 3. Decisions

### 3.1 Shown, not a grant item of its own

The first version made each pair a manifest item, `SecretSend(secret, host)`, approved at deploy
and checked again on every request. It was dropped:

- **The approval already covers it.** The hosts are each approved, and the graph fixes which
  secret goes to which before the flow runs. A version that moves a key to another host is a new
  version: an agent's version that reads a secret always waits for a person (`approval_needed`
  in `deploy.rs` asks for its `SecretRead` again whenever the unit changes), who deploys it from
  the editor and sees the flow and, now, where each secret goes. A person's own redeploy isn't
  asked again, but they wrote it.
- **A check per request would repeat that decision.** The host already refuses a host not
  granted (`blocked_host`); a pair check would only refuse what the approved graph can't do.
- **A new item costs every deployment.** On upgrade, an item a grant lacks pauses the deployment
  (`persist.rs`); every flow that calls an API with a key would have paused, or needed a
  carry-over rule.

What a separate item would add is an admin's cap ("this key may only go to api.stripe.com") and
offline evidence; §6 keeps both for later.

### 3.2 Derived from the graph, not written in the text

The pairs come from the graph, like every capability (`PLATFORM_LAYOUT.md`: capabilities are
derived "so they can't be understated"). A header form, `secret stripe_key for api.stripe.com`,
was weighed: it states the author's intent, but says again what the graph says and would still
have to be checked against it. If wanted, it can come later as an assertion the compiler checks
against the derived pairs, never as the source of a permission.

### 3.3 Granularity: the host

A pair names the host, not the part of the request (header, path, body): a host is what
`HttpRequest` grants and what a person can judge, and the far side reads the whole request. The
parts stay in the log: an exchange's withheld or redacted parts say where in the request a secret
was.

## 4. Redirects

A redirect is the one place a request can reach a host the graph doesn't name: the far side picks
it (`Location`), and the host follows it if that host is granted too (`outbound.rs`, checked on
every hop). What already held: to another host, only the headers that say what the body is go
along (`CARRIED_ELSEWHERE`), so an `authorization` header or an API key stays with the first
host; and a 303, or a 301/302 after a POST, becomes a GET without a body.

What didn't: a 307 or 308 resends the body, and a body may hold a secret as a header may (a token
exchange's `client_secret`, a signed form). Now a redirect to another host that would resend a
body isn't followed: the call fails with `blocked_redirect` ("a 307 to … would resend the body to
another host"), and the flow can call that host itself if it is granted. To the same host, the
body and headers go along as before.

## 5. What changed, and where

- `holon-core/src/taint.rs`: `SecretFed` records the hosts each followed secret reaches;
  `sent_secrets`; a test.
- `holon-author`: `describe::describe_item_sending`; `holon check` uses it; `agent::Plan::sent`
  and `agent::describe_sending`; the hover (`editing.rs`, via `commands::sent_of`).
- `crates/holon`: the deploy preview answers `sent`; `holon deploy` and the agent tools
  (`deploycmd.rs`, `mcp.rs`) say it; `outbound.rs`: `blocked_redirect`.
- `holon-wasm` and the editor: `describe_item_sending`; the deploy dialog (`useDeployFlow.ts`).
- Sample: stripe-to-slack's comment says what its approval shows. Docs: the permissions
  reference, deploying, PLATFORM_LAYOUT.

## 6. Not done, and open

- **The canvas.** Marking a request node that a secret reaches needs core's per-node answer
  mapped onto the editor's nodes. The approval and the text pane show it; the canvas doesn't yet.
- **Databases.** A secret in a read's parameters goes to the database. Rare, and a database
  binding is the admin's; it would be said the same way ("send it to the database "shop"").
- **Evidence.** `holon verify` could check, from an export, that each exchange carrying a secret
  went to a host the flow's unit sends it to. The log already names each exchange's host and node.
- **An admin's cap.** Server-wide secret bindings (folder-style: one `stripe_key` for several
  flows, a deployment's own overriding it) could name where a secret may go, and the policy
  refuse a deploy that sends it elsewhere. That is when a pair would become an item.
- **Tokens a call answers** (`access_token`, `ANSWER_FIELDS` in `secrets.rs`): refused the writes
  a secret is, but not followed as one, so not shown where they go.
