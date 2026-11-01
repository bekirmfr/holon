# Secret destinations: a secret goes only where a person approved it (L4)

**Status:** designed, not built (2026-11-02). Claims about the code are checked on
`claude/declared-names-ef7be1` after the commit that makes answering a route's caller with a
secret a permission (`SecretAnswer`), and cited by file there.

A flow's header declares the secrets it reads (`secret stripe_key`, docs/L4s/declared-names-L4.md),
and the deploy approval lists each one ("read the secret "stripe_key"") beside each host the flow
calls ("call https://api.stripe.com"). The two lists are approved separately, and nothing ties
them together: a flow approved to read `stripe_key` and to call `api.stripe.com` and
`hooks.slack.com` may send the key to either. A person approving an agent-written flow can't see,
and the evidence can't show, where a secret went. This note makes it one rule:

> **A secret leaves a run only for a destination a person approved for that secret.** Each pair,
> a secret and where it goes, is derived from the graph, listed in the approval, enforced by the
> host and checkable offline.

For payment ops, the first outside user this is aimed at (agent governance plus offline-verifiable
evidence), this is the sentence that matters: an agent-written flow can't send a key anywhere it
wasn't approved to go, and an export proves it.

## 1. Where a secret can go today

The analysis that answers this exists: `holon_core::taint` follows a secret's value through a
run's graph before it runs, as **literal** (the value as it is: a record built with it, a
concatenation) or **derived** (computed from it: base64, a slice, a field). One-way leaves
(`sha256`, `hmac_sha256`, `secure_equals`), a Boolean, and a call's answer cut the chain. It can
follow one secret alone (`SecretFed::of_secret`). Where a secret's value can end up:

| Destination | Today |
|---|---|
| The run log, exports, the editor | Redacted (literal) or withheld (derived): `secrets.rs`, `taint.rs` |
| State, collections, folders, database writes, channel packs | Refused, literal or derived: `firing.rs` |
| Another flow's action (`calls billing`) | Refused: `calls.rs` |
| An action's answer | Withheld and redacted: `deploy.rs` |
| A route's answer | Only where approved, `SecretAnswer("name")`: `listeners.rs` |
| **An outbound HTTP request** (`http.get/post/…`, uploads, streams), its path, headers or body | **Any host the flow is granted** (`outbound.rs`, `Outbound::check` checks the host only) |
| **A redirect** of such a request | Followed to any granted host with the same headers |
| **A database read's parameters** (`d.query("… $1", secret.x)`, a Mongo filter) | **Sent to the database**: reads aren't refused |

The last three are the gap. HTTP is the one that matters now; database parameters are rarer, and
a database binding is the admin's.

## 2. The rule

A **destination** is where a request takes a value outside the host: an HTTP host (`host` or
`host:port`, as `HttpRequest` names it), or a bound database (as `DbRead` names it, by its name
for the flow). Route answers already have their item.

A flow **sends a secret to a destination** when, in some run, the secret's value or something
computed from it may reach a request to it. That is derived, per secret, by the walk that exists:
for each request node (`http_request`, `http_upload`, `http_stream`; later `db_query`, `db_one`,
Mongo reads), the per-secret taint of its inputs (path, headers, body; a query's values) is
literal or derived. The node's host is a frozen param (`kind.rs`, `http_host`), so every pair is
known before the flow runs, as every other capability is.

Each pair is a manifest item, a capability core derives:

```
SecretSend("stripe_key", "api.stripe.com")       send the secret "stripe_key" to api.stripe.com
SecretSend("slack_webhook_path", "hooks.slack.com")
```

`deployed_capabilities` (core, `taint.rs`) adds them as it adds `SecretAnswer`. It is already
where the server, the deploy preview, `holon check`, the agent tools and the editor (wasm) get a
manifest, so all of them show the pairs with no second rule.

## 3. Decisions

### 3.1 Derived from the graph, not written in the text

The pairs come from the graph, like every capability (`PLATFORM_LAYOUT.md`: capabilities are
derived "so they can't be understated"). The author writes nothing new, and can't write something
the graph contradicts. A header form, `secret stripe_key for api.stripe.com`, was weighed:

- *For:* the author's intent, stated where the secret is declared; a mistake (the key sent to the
  wrong host) is a compile error at the use, not a line in the approval.
- *Against:* it says again what the graph says, and a text that declares one thing and does
  another must still be checked against the graph, so derivation is needed either way.

**Lean:** derived only, now. A `for` clause can come later as an assertion the compiler checks
against the derived pairs (§7), and never as the source of the permission.

### 3.2 Approved like every item, capped by the host

A person approves each pair at deploy, as they approve hosts and secrets. Since a pair names both,
the approval can say it in one line under the secret, not three:

```
read the secret "stripe_key", and send it to api.stripe.com
```

The admin may also **cap** where a secret may go, on the host: a secret's binding names the
destinations allowed (§4). The policy then refuses a deploy whose pairs go outside them, saying
which, as it refuses plain HTTP the host never grants (`permissions.rs`, `refuses_item`). The
author proposes (by the graph), a person approves (at deploy), the admin bounds (on the host).

### 3.3 Granularity: the host

A pair names the host, not the part of the request (header, path, body) nor a path prefix.

- A host is what `HttpRequest` grants, what the outbound check enforces, and what a person can
  judge.
- "Only in the `Authorization` header" sounds stricter but isn't safer: the far side reads the
  whole request. And it would make the common case wordy.

The parts stay in the log: an exchange's withheld or redacted parts say where in the request a
secret was. **Open:** a path prefix (`api.stripe.com/v1/`) for a host that serves several
tenants; see §7.

### 3.4 Enforced at the request, not only at deploy

The deploy approval is the gate; the host enforces it again for each request, as it does for hosts
(`blocked_host`):

- **By the graph:** a request node whose inputs the secret reaches (the run's `SecretFed`, per
  secret) may go only to a host its pairs grant. Otherwise the request isn't sent, and it fails as
  `blocked_secret`, logged like `blocked_host`.
- **By the value:** a request holding a secret's value as it is (`secrets_in`, as the route
  answer check uses) to a host not granted for it is refused the same way. This catches a value
  the graph can't follow: one read back from a call's answer, say.
- **Redirects:** a request carrying a secret follows a redirect only to a host granted for that
  secret. Today it follows to any granted host with its headers (`outbound.rs`); that is the hole
  this closes.

### 3.5 Evidence: checkable offline

The grant records its items and its hash, and every log entry names the grant in effect. Each
outbound exchange is logged with its host and its node's path, and with each secret part redacted
(`‹secret:stripe_key v3 #…›`) or withheld. `holon verify` can then check, from an export alone:

- **from the unit:** the pairs, derived by core, the same function the host used;
- **from the log:** each exchange that carried a secret, named by its redaction mark or its
  node's taint, went to a host the grant in effect allowed for it.

A verify report gains a line, "every secret went only where its grant allowed". A run where one
didn't can't exist unless the host was tampered with, which is what verify is for.

## 4. Host-side binding of a secret

The value already lives on the host:

- sealed, versioned and audited (`secrets.rs`);
- or a reference the host resolves (`secret_sources.rs`: `env:`, a dotenv file, `vault:`).

It is per deployment only. Folders show the shape to grow into (`folders.rs`): a binding is for
the whole server or for one deployment, and the deployment's own binding of a name wins
(`Scoped`). For secrets that gives:

- **Server-wide bindings:** one `stripe_key` that several flows read, set and rotated once. A
  deployment's own binding overrides it.
- **A binding's destinations:** `holon secret set shop stripe_key --to api.stripe.com` (or the
  server-wide form, or the ⚙ panel). A binding with no destinations caps nothing; the approval
  alone governs.
- **The audit** records destination changes as it records versions.

This is the admin's half of §3.2, and it can land after the pairs without changing them.

## 5. Migration

On upgrade, a deployment's manifest is derived again at load (`persist.rs`), and an item its grant
lacks pauses it ("it needs … it isn't granted"). Pausing every flow that calls an API with a key
would be the wrong first impression. So:

- **Carried over on upgrade:** a pair `SecretSend(s, h)` is granted at load when the grant already
  holds both `SecretRead(s)` and `HttpRequest(h)`. The person approved both, and before this rule
  that implied the pair. The audit and the grant's history say so ("granted on upgrade: implied by
  …"). Nothing pauses.
- **Asked from then on:** a new deploy, or a version that adds a pair, asks for it like any new
  item.
- **`SecretAnswer`, already built, needs the same decision:** today a deployment whose route
  answers with a secret pauses after the upgrade until a person grants it (or deploys it again).
  It could be carried over the same way (from `SecretRead` plus `NetworkIngress`), or left
  pausing because answering with a secret is rarer and riskier. **Lean:** leave it pausing, said
  in the release notes, since the whole point of that item is that nobody approved it.

## 6. What changes, and where

- `holon-core`:
  - `Capability::SecretSend(String, String)`, a sink in the saturated set;
  - `taint.rs`: `sent_secrets(root, resolver) -> BTreeSet<(secret, destination)>`, beside
    `answered_secrets`, and added in `deployed_capabilities`;
  - `kind.rs`: the request leaves' hosts, read as `http_host` reads them.
- `holon-author`:
  - `describe.rs`: "send the secret "s" to h";
  - `holon check`: grouped under each secret;
  - `editing.rs`: hover on `secret.s` names where it goes.
- `crates/holon`:
  - `outbound.rs`: `blocked_secret`, by the graph and by the value, and redirects;
  - `persist.rs`: the carry-over;
  - `permissions.rs`: the host's cap from a binding's destinations;
  - `verify.rs`: the evidence check;
  - `secrets.rs` and `secret_sources.rs`: server-wide bindings and destinations (§4).
- `holon-wasm` and the editor: the item crosses as the others do; the approval groups a secret's
  lines.
- Samples: those that send a secret (stripe-to-slack, chat-proxy, gemini-chat, guarded-api) show
  the pairs in their approval. One sample shows a refused send: a key bound to one host, and a
  route that tries another, answering 502 with `blocked_secret` in its log.
- Docs: the permissions reference, deploying, PLATFORM_LAYOUT §4.2, the manual's checking-runs
  page for verify.

## 7. Slices

1. **Pairs, approval and enforcement for HTTP.**
   - The capability, its derivation, its wording, check and the editor.
   - The request check, by graph and by value, and redirects.
   - The upgrade carry-over.
   - Samples updated.
2. **Evidence.** `holon verify` checks each exchange that carried a secret against the grant in
   effect.
3. **Host bindings.** Server-wide secret bindings with a deployment's override, and destinations
   that cap what a deploy may ask (CLI, ⚙, audit).
4. **Databases.** A secret in a read's parameters, as `SecretSend(s, "db:shop")` or a form of its
   own.
5. *(Later, if wanted.)* `secret stripe_key for api.stripe.com` in the header as an assertion the
   compiler checks against the derived pairs.

Each slice is one commit, with its sample run live, as CLAUDE.md asks.

## 8. Open questions

- **Path prefixes** for multi-tenant hosts (one host, many customers' paths): a pair per prefix
  would need the path to be static, and it usually isn't (`/v1/customers/{id}`).
- **Tokens a call answers** (`access_token` from an OAuth exchange, `ANSWER_FIELDS` in
  `secrets.rs`): they are refused the writes a secret is, but not followed as one. Should they be
  bound to the host that issued them? Likely yes, as a pair derived from the call that answered
  them; it needs the taint walk to start at that call's answer.
- **The database destination's name**: `db:shop` in the same item, or `SecretQuery("k", "shop")`.
- **A secret given to a sandboxed command** (`host-capabilities-L4.md` §14): everything that comes
  out is derived. Its destinations are the sandbox's proxy hosts, so the same pairs apply there.
- **Policy auto-grants**: an admin's pattern `SecretSend` would grant every pair. Should a pair be
  auto-grantable at all, or only capped?
