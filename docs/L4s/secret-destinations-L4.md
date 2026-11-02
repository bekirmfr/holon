# Secret destinations: where a secret goes is shown where a person decides (L4)

**Status:** built (2026-11-02), on `claude/declared-names-ef7be1`, in the commits from
"secrets: the approval says where each secret is sent" to "secrets: the server's own secrets,
and where an admin says each may go". The first version of this note proposed a grant item per
secret and host, enforced on every request; §3 says why that was dropped, and §6 how an admin's
cap came back where it belongs, on the server's own secrets. Claims about the code are cited by
file.

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
| An outbound HTTP request (path, headers, body) | To the hosts the flow names and a person approved; **the approval says which secret goes to which** (§2), and the host holds a run to it (§5) |
| A redirect of such a request, to another host | Without the flow's headers but those that say what the body is (`CARRIED_ELSEWHERE`), and not followed when it would resend a body (§4) |
| A database read's parameters (`d.one("… $1", secret.x)`) | Sent to the database, and said so: "send it to the database "vendor"" (§2) |
| What a call answers to it (a token, an echo) | Followed, and said: "what is answered to it goes to https://api" (§2) |

## 2. Where each secret is sent, derived

A flow **sends a secret to a host** when the secret's value, or something computed from it, may
reach an outbound request to that host: its path, headers or body. Core derives it,
`taint::sent_secrets(root, resolver) -> secret → destinations`, by the walk that exists: a
request leaf (any leaf whose capability is `HttpRequest(host)`: `http_request`, `http_upload`,
`http_stream`) fed a tainted input sends each followed secret to its host, and a node the walk
doesn't follow (a `map` over a function that reads one, a module) counts every host it could
call. A request's host is a frozen param, so every pair is known before the flow runs.

Two more kinds of destination are listed with the hosts (`taint::destination_of`):

- **A database**, `db:vendor`: a read whose `$1`, `$2`, … a secret reaches (`db_query`,
  `db_one`, a MongoDB find) sends it to the database's server with the query. A write can't
  carry one: the host refuses to keep a secret. A read's rows, like a request's response, are
  what the far side answered, so they cut the chain (an echo of the value is still found by it).
- **What is answered to it**, `answer:httpbin.dev`: a second walk, `SecretFed::answers_of`,
  starts from the answers of the requests and reads the secret was sent in (the access token a
  token exchange answers, an API's echo), the secret itself clean, an answer to what was
  answered followed too. The graph can't tell a token field from the rest of an answer, so it
  follows the whole answer; the host tells tokens by their field (`ANSWER_FIELDS`, `secrets.rs`).

It is said beside the secret's item, by one function (`describe::describe_item_sending`):

```
read the secret "slack_webhook_path", and send it to https://hooks.slack.com
read the secret "stripe_webhook_secret"
read the secret "license_key", and send it to the database "vendor"
read the secret "client_secret", and send it to https://demo.duendesoftware.com; what is answered to it goes to https://httpbin.dev
```

(stripe-to-slack: the Stripe secret only verifies a signature, so it is sent nowhere;
license-check and client-credentials are the other two.) Where:

- the editor's deploy approval, from the deploy preview's `sent` (`deploy.rs`), both under "It
  needs your approval to" and "Already granted";
- `holon check`, `holon deploy` (its "It asks to" and "approved, as you" lists) and the agent
  tools' deploy and save answers (`agent::Plan::sent`);
- the editor's hover on `secret.x`, and on `secret x` in the header: "This flow sends it to
  https://hooks.slack.com";
- the canvas: every node where a secret's value leaves the computation is marked 🔑 with what it
  does with it (`flow::secret_marks`, from `SecretFed::sinks`): a request it is sent in, a
  database read, a write or a call the host refuses, the route's answer, and where what is
  answered to it goes. Core's paths are in canonical child order; `nodes_of` maps them onto the
  canvas's nodes.

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
offline evidence. Both are built without one: evidence in §5, the cap in §6.

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

## 5. Held at run time, and checked offline

The graph says where a secret goes; a value it can't follow (one a call answered back and the
flow passed on, one that came in from outside) is held by its value:

- **At run time**: a request holding a secret's value bound for a host the secret isn't shown
  going to isn't sent; it answers `{status: 0, error: "blocked_secret", message}` naming the
  secret, never its value (`folders::SecretSends`, built in `firing.rs` when a run reads one).
- **Offline**: `holon verify` checks, from an export, that every exchange whose request carries
  a secret's mark went to a host its unit sends that secret to, and every exchange carrying a
  token's mark to a host the unit sends what is answered to that secret to (a token is tied to
  the secrets of the request that got it by its fingerprint). Requests refused before they left
  went nowhere. The report says "secrets: N outbound requests carried one, each to a host its
  approval shows it going to", and the same for tokens, or names each run that SENT one
  elsewhere.

## 6. The server's own secrets, and an admin's cap

A secret bound once for the server (`holon secret bind stripe_key`, or admin → resources →
secrets) is read by every deployment that reads a secret of that name and has none of its own; a
deployment's own overrides it. It is kept sealed in the same store, as `secrets::SERVER`'s.

There the cap belongs: the key is the admin's, not the flow author's. `--to api.stripe.com`
says where it may go, in `sent_secrets`' terms (hosts, `db:` databases). A deploy whose flow
sends it elsewhere is refused before anything is approved (`SecretStore::refuses`, in the
preview and at deploy), so no grant item was needed: the cap isn't a permission a person
approves, it is what the host won't do. A deployment with its own isn't held to it. A cap set or
narrowed after a flow was deployed holds from its next request (`SecretSends` checks the
server's values against it). Binding, capping and removing are audited, never a value. Sample:
shared-key.

## 7. What changed, and where

- `holon-core/src/taint.rs`: `SecretFed` records where each followed secret goes and each sink
  it reaches, by node; `answers_of`; `sent_secrets`, `destination_of`, `ANSWER`; a database
  read's rows cut the chain.
- `holon-author`: `describe::describe_item_sending`, `describe_sent_to`, `describe_secret_sink`,
  `describe_answer_sink`; `holon check`; `agent::Plan::sent`; the hover; `flow::secret_marks`.
- `crates/holon`: the deploy preview answers `sent`; `holon deploy` and the agent tools say it;
  `outbound.rs`: `blocked_redirect`; `folders::SecretSends`: `blocked_secret`; `verify.rs`:
  `check_sends`; `secrets.rs`: the server's secrets and caps; `holon secret bind / cap / bound /
  unbind`; `/admin/secrets`.
- `holon-wasm` and the editor: the deploy dialog, the canvas marks, the admin page's secrets, a
  deployment's settings.
- Samples: stripe-to-slack, echoed-key, license-check, client-credentials, shared-key. Docs: the
  permissions reference, deploying, checking runs, the editor, databases, the command line,
  PLATFORM_LAYOUT.

## 8. Open

- **The cap covers the secret, not what is answered to it.** A token got with the server's key
  may go wherever the flow's approval shows. A cap on it too is a small step (`answer:`
  destinations against a second list), once someone needs it.
- **A cap doesn't name a deployment.** It holds for every deployment that reads the server's
  key; one that needs it elsewhere sets its own.
- **No run-time check for databases.** A query's parameters go where the graph says, to the
  database the admin bound; there is no value check on them as there is on a request.
