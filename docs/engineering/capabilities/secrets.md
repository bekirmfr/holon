# Secrets

Named values a flow reads but never keeps: an API key, a signing secret, a webhook path. What a
secret is, where it is kept and how its value stays out of everything Holon writes is in
`crates/holon/src/secrets.rs`; where it may go is the
[secret destinations L4](../../L4s/secret-destinations-L4.md); the user's side is the manual's
[server reference](../../manual/reference/server.md) (the secrets key) and
[permissions](../../manual/reference/permissions.md).

## 1. What it is

A flow declares a secret in its header and reads it by name ([LANGUAGE.md](../../LANGUAGE.md) §5.1,
§5.4):

```
flow sign-payload
secret signing_key

route POST /sign (req) {
  respond { signature: hmac_sha256(secret.signing_key, req.body, encoding: hex) }, missing: 400
}
```

`secret.signing_key` is the `secret{name: signing_key}` leaf, capability `SecretRead("signing_key")`,
granted at deploy like any other; `optional secret receipt_key` lets the deploy leave it out, its
reads then without a value. A use the header doesn't declare is an error, and `fmt` declares it.
The value is set for the deployment (the editor's ⚙, `holon secret set`), or once for the server
(`holon secret bind`), never in the flow.

- `secrets.rs`: the store (`SecretStore`), a deployment's values in memory (`Vault`, `Resolved`),
  the marks and the `Redactor`.
- `sealing.rs`: encryption at rest and the master key (`Sealer`, `KeySource`).
- `secret_sources.rs`: values kept elsewhere (`Reference`, `Sources`).
- `holon-core/src/taint.rs` and `crates/holon/src/taint.rs`: where a secret's value flows in the
  graph (`SecretFed`, `sent_secrets`).
- `secretcmd.rs`: `holon secret list|history|set|rollback|remove|bind|cap|bound|unbind|env`.

## 2. How it works

- **Stored sealed.** The secrets file (`--secrets`, `secrets.json`, owner-only, written whole by
  `write_private`) keeps each deployment's secrets as versions (`Stored`: version, time, source,
  nonce, sealed value), never a value in the clear. Each value is sealed with AES-256-GCM under
  the master key, a fresh nonce each, the slug, name and version as associated data, so a sealed
  value moved elsewhere fails to open. The file records `key_check` (derived from the key) and
  `key_id`, so a wrong key is told from a damaged file and the server refuses to start rather than
  start empty over the real store.
- **The master key** is kept apart from the file: `--secrets-key os` (the OS key store, the
  default on Windows and macOS), `env:VAR` (64 hex digits, never created; `--new-secrets-key`
  prints one) or `file:PATH` (owner-only, the default elsewhere, in `~/.holon/keys`, outside the
  data folder) (`KeySource::parse`).
- **Or a reference.** A version's source is `local` (sealed here), `env:HOLON_SECRET_<NAME>`
  (only `ENV_PREFIX` names: the server's environment holds credentials no flow may ask for) or
  `vault:<mount>/<path>#<field>` (HashiCorp Vault KV v2). `--dotenv FILE` adds a file's
  `HOLON_SECRET_*` lines, the real environment winning; the editor lists the names, never values
  (`Sources::names`). References are read again every `REFRESH` (60 s); a value that can no
  longer be read is kept `STALE_AFTER` (300 s) at most.
- **Versions and audit.** A set adds a version; `MAX_VERSIONS` (5) are kept; a rollback makes an
  old one current as a new version. Every change, and a source that stops or starts reading, is a
  line of `<secrets>.audit.jsonl`: what, when, version and source, never a value. A value under
  `MIN_SECRET_LEN` (6) is refused: it would match ordinary text.
- **The server's own** are kept under `SERVER` (`*`) and read by a deployment with none of its
  own of that name; `--to` caps where one may go (`SecretStore::refuses` at deploy).
- **Redacted.** `Redactor::of(vault)` pairs each current and retired value, and its JSON-escaped
  form, with its mark, `‹secret:name v3 #a1b2c3d4›` (`mark`: the version and an 8-hex fingerprint
  keyed by a key derived from the master key). `Redactor::entry` marks everything a log entry
  carries (request, sourced values, state before, outputs, effects, reads, timeline, rounds,
  answer, error, url) and escapes a run's own look-alike of a mark (`write_marked`). Control-plane
  replies go through `Redactor::json`; exports carry the log's lines, already marked. Credentials
  a call answered to a request that held a secret (`ANSWER_FIELDS`: `access_token`, `token`, …)
  read `‹token:name #…›` (`Redactor::answers`), resume tokens `‹token:name›`.
- **Followed.** Core's taint walk follows a secret's value through the graph as literal or
  derived, cut by a one-way leaf (`sha256`, `hmac_sha256`, `secure_equals`), a Boolean or a call's
  answer. What is derived from one is withheld from the log (`WITHHELD`); a state, collection,
  file, database or channel write of a secret or of what is derived from one is refused
  (`firing.rs`); a call to another flow's action carrying one is `refused` (`calls.rs`); a route
  answers with one only where `SecretAnswer("name")` is granted.
- **Where it goes.** `taint::sent_secrets` names each secret's destinations (hosts, `db:` for a
  database read's parameters, `answer:` for what a call answers to it); the approval says them
  ("read the secret "slack_webhook_path", and send it to https://hooks.slack.com",
  `describe_item_sending`). At run time `SecretSends::refuses` turns away a request holding a
  secret's value bound elsewhere (`blocked_secret`); offline `holon verify` checks every marked
  exchange went where its approval shows (`check_sends`).

## 3. Why it is this way

- **A file apart from the deployments file, sealed under a key kept elsewhere**: a backup, a disk
  image or a zipped data folder carries nothing readable (sealing.rs module docs); a backup of
  `deployments.json` carries none either, since a waiting run's requests are sealed too
  (`persist_deployments`).
- **Fingerprints, not values, in the log**: which value a run had (an old key, the new one) can
  be told, and a run still checks after its secret rotates (rc.28), without the log holding it.
- **References for values another system owns**, so rotating it there needs nothing here.
- **Destinations shown, not a grant item.** A per-pair item `SecretSend(secret, host)` was
  dropped: the graph already fixes which secret goes to which host, a check per request would
  repeat the approval, and a new item would have paused every deployment on upgrade
  ([secret destinations](../../L4s/secret-destinations-L4.md) §3.1). The cap came back where it
  belongs, on the server's own secrets (§6 there).
- **Redaction only when it can matter.** With nothing to mark, the line is made and looked at for
  `‹`, redacted only if it has one: the `notes` listing spent 19.2k of its 463.8k instructions in
  redaction before that step ([interpreter path L4](../../L4s/interpreter-path-L4.md) step 6). A
  debug-build tripwire holds the shortcut to redacting first (D3,
  [resilience audit](../../L4s/resilience-audit-L4.md) §2.7).

## 4. Reach and limits

- **Redaction catches literal values** (and their JSON escaping), nothing transformed: a value
  hashed, encoded or sliced isn't recognised by value. The taint walk withholds what the graph
  derives; a value it can't follow (echoed back by an API and passed on) is caught only while it
  is literal.
- **Token fields by name.** An answered credential under another field name is followed by the
  graph but not marked as a token.
- **No run-time check on a database's parameters**: they go where the graph says, to the
  database the admin bound (secret destinations §8). A cap covers the server's secret, not what is
  answered to it, and names no deployment.
- **A revoked Vault value works for up to `STALE_AFTER`**; a source read fails loudly, once, in
  the audit.
- **In memory in the clear** while the server runs (`Zeroizing`, wiped when dropped).
- **`--secrets :memory:`** keeps nothing across a restart: a waiting run's requests are sealed in
  a persisted deployments file under a key that is never kept, so after a restart (the secrets
  gone with it) the run can't be opened, and says so.

## 5. What the tests hold

- secrets.rs: `a_store_keeps_values_sealed_and_survives_a_restart`,
  `a_missing_or_wrong_key_refuses_instead_of_starting_empty`, `versions_are_kept_and_old_values_still_redacted`,
  `a_mark_names_the_version_and_a_fingerprint_of_the_value`, `a_vault_value_follows_its_source_and_outlives_an_outage_only_briefly`,
  `a_rollback_restores_an_older_version_as_a_new_one_and_everything_is_audited`,
  `redaction_replaces_every_literal_occurrence`, `a_look_alike_of_a_mark_is_escaped_and_reads_back_as_text`,
  `with_nothing_to_mark_a_text_without_a_mark_sign_stays_as_it_is`,
  `the_servers_secret_is_read_where_a_deployment_has_none_and_keeps_its_cap`.
- sealing.rs: `a_sealed_value_opens_only_where_it_was_sealed`; secret_sources.rs:
  `references_parse_and_only_holon_variables_can_be_named`, `a_dotenv_file_reads_like_the_usual_tools`.
- tests.rs: `a_secret_reaches_its_runs_and_nothing_else`, `a_secret_computed_into_something_else_stays_out_of_the_log`,
  `what_is_computed_from_a_secret_is_neither_kept_nor_logged`, `a_secret_goes_only_where_its_approval_shows_it_going`,
  `the_servers_secret_goes_only_where_an_admin_said_it_may`, `a_token_a_call_answers_with_a_secret_is_kept_out_of_the_log_and_out_of_state`,
  `a_route_answers_with_a_secret_only_where_a_person_approved_it`, `a_run_shows_which_secret_value_it_had_and_still_checks_after_it_rotates`,
  `a_secret_stays_out_of_files_and_of_what_the_log_keeps_of_them`, `a_secret_is_never_sent_to_another_flow`.
- **Samples**: `guarded-api` (sources, versions, rollback), `sign-payload` (verify with a secret),
  `stripe-to-slack`, `echoed-key`, `client-credentials`, `license-check` (where each goes),
  `browser-key` (`SecretAnswer`), `shared-key` (the server's, capped). All deploy in
  `samples.e2e.ts`, and each of `browser-key`, `stripe-to-slack`, `license-check`,
  `client-credentials`, `shared-key` and `guarded-api` has a test there, as does an env secret
  picked from `HOLON_SECRET_…` names with its dotenv file.
- **Gaps**: CI runs on Linux only, where the default is a key file: the OS key store (the
  default on Windows and macOS) isn't exercised there; redaction's cost with many secrets isn't
  measured.

## 6. History

The store, sealing, sources, versions and the audit predate this repository's history (PLATFORM_LAYOUT
§4.6 step 7; `key_id` rc.17; env picked from a list rc.27; fingerprints and answered tokens rc.28,
in the CHANGELOG). Since the rc.29 import (`6bc5c68`):

| Commit | Change |
|---|---|
| `8f93149` | `holon secret` and the other page actions as commands |
| `2c15b96` | Answering a route's caller with a secret is a permission (`SecretAnswer`) |
| `14c7163` | The approval says where each secret is sent |
| `f3b834d` | A redirect elsewhere doesn't resend a body that may hold one |
| `21e63e6` | A secret goes only where its approval shows (`blocked_secret`), and verify checks it |
| `75ea56d` | The canvas marks every sink a secret reaches |
| `ded3615`, `a053b40` | A database read's parameters, and what a call answers, as destinations |
| `71b895c` | The server's own secrets, and an admin's cap |
| `6220b50` | Redaction without a copy |
| `f47c854` | Redaction looked at, not walked, with nothing to mark |
| `cf1f961` | D3: the redaction shortcut checked against redacting first |
