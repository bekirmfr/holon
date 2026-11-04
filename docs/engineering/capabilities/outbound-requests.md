# Outbound requests

Calls a run makes out to HTTP(S) APIs: `http.get("httpbin.org", "/json")` and its kin. The rules
are in [PLATFORM_LAYOUT](../../PLATFORM_LAYOUT.md) §4.1–§4.2 (rounds, the rules, limits and
exceptions); where a secret may go is the [secret destinations L4](../../L4s/secret-destinations-L4.md);
what earns a new kind of reach is the [host capabilities L4](../../L4s/host-capabilities-L4.md).

## 1. What it is

A request is a leaf whose capability is `HttpRequest(host)`, the host a frozen param (a text
literal, so it is known before the flow runs): `http_request{method, host}(path, headers, body)`,
`http_upload` (a file sent by handle, `file: h`) and `http_stream` (an answer forwarded as it
arrives, `stream: true`, or piped to a channel, `updates.pipe(http.post(…))`) (LANGUAGE.md
§5.4). Evaluation stays pure: a request is answered between rounds, by the host, and recorded.

- `holon-host/src/rounds.rs`: the round loop, the `Performer` trait, `Limits`, `perform_all`.
- `crates/holon/src/outbound.rs`: `Outbound`, the performer that sends, and every rule.
- `crates/holon/src/folders.rs`: `Sender`, a deployment's run performer, which adds a file body,
  a stream's forwarder, a pipe, calls to other flows' actions and the secret check
  (`SecretSends`) in front of `Outbound`.
- `crates/holon/src/permissions.rs`: the `Outbound` a permit holds, from the grant and policy.

## 2. How it works

- **Rounds.** A run is several evaluations of one flow. In each, an `http_request` whose request
  is ready asks for its answer; the round's unanswered requests go out together, and the flow is
  evaluated again with their answers, keyed by node path and request hash. A round that asks for
  nothing new is the last: only its outputs and effects count (`rounds.rs` module docs).
- **Out at once, within bounds.** `perform_all` groups a round's requests by host, hosts in
  parallel, at most `per_host_concurrency` (6) at once to one; calls to another flow's action go
  one at a time in node order. No request outlasts the run: each round's `request_timeout` is cut
  to what `run_budget` has left.
- **The rules,** at perform time whatever the flow says (`Outbound::check`, `send`): a host the
  grant revoked answers `revoked`, one not granted `blocked_host`, plain `http://` to a host not
  granted `PlainHttp` `blocked_scheme`. A URL with credentials (`user@`) or another scheme is
  `bad_url`. The host sets `Host` and the framing headers (`HOST_HEADERS`, and any `proxy-`): a
  flow that sets one gets `bad_header`. Private, loopback, link-local, carrier-grade NAT and
  metadata addresses (`is_private`: an IPv4 address inside IPv4-mapped, IPv4-compatible, 6to4 and NAT64 forms
  judged as itself, Teredo refused whole) are filtered from what DNS
  returned, in the client's resolver, so the connection uses only what passed (`blocked_address`).
- **Redirects** are followed by hand, up to `max_redirects`, each hop checked again. To another
  host only `CARRIED_ELSEWHERE` (`content-type`, `accept`, `accept-language`, `user-agent`) goes
  along; a 303, or a 301/302 after a POST, becomes a GET without a body; one that would resend a
  body to another host isn't followed (`blocked_redirect`).
- **The answer** is `{status, headers, body}` within `max_header_bytes` and `max_response_bytes`;
  `body` is text. A file body is streamed from its folder, hashed as it goes, and must still match
  the handle (`changed`); a streamed 2xx answer goes to a `Forward` piece by piece while the whole
  is kept for the run.
- **Failures are answers.** A request that breaks a rule or a limit, or fails, answers
  `{status: 0, error, message}` (`failure`; `transport_failure` maps timeouts, `dns`, `connect`),
  its message without the URL (`without_url`: a query may hold a secret). The flow decides. Only
  the run's own budgets fail the run: `RunError::RequestBudget` past `max_requests` (50),
  `RunError::TimeBudget` past `run_budget` (25 s for a request's run, 60 s for a clock's);
  then nothing is performed.
- **Recorded.** Every exchange (node, request, response, times) is in its round (`Exchange`,
  `Round`) and in the run's log entry, redacted. A replay, `holon verify` and a run that runs
  again (its reads changed before it committed) answer from that record (`prior`), so a run asks
  the outside once whatever it does after.
- **Secrets.** Before `Outbound` sees it, `SecretSends::refuses` turns away a request holding a
  secret's value bound for a host its approval doesn't show (`blocked_secret`); see
  [Secrets](secrets.md).

## 3. Why it is this way

- **Answers in rounds, not callbacks.** Evaluation stays pure and total, so a run replays offline
  from its record and `verify` needs no network (PLATFORM_LAYOUT §4.1).
- **The host is the grant.** It is the unit a person can judge and what a flow's text fixes; a
  host from a wire is a compile error (LANGUAGE.md §5.4, rules).
- **Checked on resolved addresses**, not names, so DNS rebinding can't reach a private address
  (outbound.rs module docs). Plain HTTP and private addresses are exceptions a deployment asks
  for and the policy must allow (`allow_plain_http`, `allow_private_addresses`).
- **A redirect is the one place a request reaches a host the graph doesn't name**: credentials
  were already left behind (rc.28); a resent body could carry a secret too, so it isn't resent
  (`f3b834d`, [secret destinations](../../L4s/secret-destinations-L4.md) §4).
- **Errors as values**: a webhook that fails shouldn't fail the run that would have handled it;
  `respond page.body, missing: 502` is the flow's choice.

## 4. Reach and limits

- **HTTP(S) only**, request and answer within one run. Connections that outlive a run (a feed, a
  session, a socket) aren't covered ([host capabilities](../../L4s/host-capabilities-L4.md) §5.9).
- **The body is text.** A response body is read as UTF-8 with invalid bytes replaced
  (`String::from_utf8_lossy`): a binary answer is mangled. There is no bytes value (§1 there). A
  binary file can be *sent*, by handle.
- **No connection reuse.** `send` builds a `ureq` agent per request, so each call connects (and
  shakes hands) afresh; a round runs a thread per request. Nothing measures what that costs.
- **No retry.** A failed call is answered once; the flow, or the caller, tries again.
- **Defaults** (`Limits::request_run`, `clock_run`; ceilings `Limits::ceilings`): connect 10 s
  (30), request 30 s (300), 50 requests (1,000), 6 per host (32), request and response 10 MiB
  (100), headers 64 KiB (256), 10 redirects (20). The manual's table
  ([permissions](../../manual/reference/permissions.md)) is the user's copy.
- **What is recorded is the record.** A response's whole body is kept in the log (redacted), so a
  large answer makes a large line; nothing measured here.

## 5. What the tests hold

- outbound.rs: `every_rule_is_enforced_at_perform_time` (`blocked_host`, `blocked_scheme`,
  `blocked_address`, `bad_header` for `Host` and the framing headers, `bad_url`),
  `limits_turn_into_error_responses`, `redirects_are_followed_and_each_one_checked`,
  `a_redirect_to_another_host_leaves_the_credentials_behind`, `a_failed_call_says_why_without_its_url`,
  `a_file_body_is_sent_whole_and_must_still_be_the_file_the_run_read`,
  `private_addresses_are_recognised`.
- rounds.rs: `dependent_requests_take_a_round_each_and_only_the_last_round_performs`,
  `the_request_budget_fails_the_run_and_performs_nothing`, `the_time_budget_fails_the_run`,
  `a_rounds_requests_to_one_host_respect_its_concurrency_limit`, `a_request_in_a_branch_not_taken_is_not_sent`,
  `a_request_inside_map_is_sent_and_answered_per_item`, `calls_to_one_flow_go_one_at_a_time_in_order`.
- tests.rs: `a_deployed_request_is_performed_between_rounds_and_logged` (127.0.0.1 refused, the
  refusal logged as the round), `granted_exceptions_let_a_request_reach_a_local_server`,
  `a_revocation_can_keep_the_deployment_running_with_the_revoked_calls_failing`,
  `a_slow_run_holds_up_no_other_and_retries_without_resending`, `a_racing_pair_on_one_key_calls_out_once`,
  `a_secret_goes_only_where_its_approval_shows_it_going`, `a_streamed_reply_whose_call_fails_answers_with_its_status`,
  `a_piped_call_sends_its_answer_to_the_channel_as_it_arrives`.
- **Samples**: `fetch-json` (a call and its host permission), `cache-aside` (a call not made),
  `idempotent` and `refund-once` (one call per key or row), `drop-box` (`http_upload`),
  `chat-proxy` and `gemini-chat` (`http_stream`, `pipe`), `client-credentials` and `echoed-key`
  (where a secret and its answer go), `stripe-to-slack`. All deploy in `samples.e2e.ts`;
  `cache-aside`, `chat-proxy`, `gemini-chat`, `drop-box` and `client-credentials` have tests of
  their own there, `refund-once` in `system.e2e.ts`.
- **Gaps**: the e2e samples call public hosts (httpbin, the APIs), so a test may pass on a
  failure answer (the drop-box test says so); DNS rebinding itself is held by the resolver's
  design, not a test that rebinds.

## 6. History

The round loop, the `ureq` performer and the rules predate this repository's history (the
PLATFORM_LAYOUT §4.6 build order, steps 3–5; `http_stream` rc.23; the host's framing headers
rc.25; credentials left behind on a redirect rc.28, all in the CHANGELOG). Since the rc.29
import (`6bc5c68`):

| Commit | Change |
|---|---|
| `14c7163` | The approval says where each secret is sent |
| `f3b834d` | A redirect elsewhere doesn't resend the body (`blocked_redirect`) |
| `21e63e6` | A request holding a secret goes only where its approval shows (`blocked_secret`), and `verify` checks it |
| `a053b40` | Where what a call answers to a secret goes is said, and verified |
| `71b895c` | A request carrying the server's own secret goes only where an admin's cap says (`blocked_secret`) |
