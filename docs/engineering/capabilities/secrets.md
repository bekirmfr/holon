# Secrets

Named values a flow reads but never keeps: an API key, a signing secret, a webhook path. What a
secret is, where it is kept and how its value stays out of everything Holon writes is in
`crates/holon/src/secrets.rs`; where it may go is the
[secret destinations L4](../../L4s/secret-destinations-L4.md); the user's side is the manual's
[server reference](../../manual/reference/server.md) (the secrets key) and
[permissions](../../manual/reference/permissions.md).

## 1. What it is

A flow imports a secret by its name and reads it as the name it imported it as
([LANGUAGE.md](../../LANGUAGE.md) §5.1, §5.4):

```ts
import { base64, flow, hmac_sha256, route, status } from "holon"
import signing_key from "secret:signing_key"

flow("sign-payload", { port: 8107 })

route.post("/sign", (req) => {
  return (
    { signature: hmac_sha256(signing_key, req.body), body_base64: base64(req.body) } ?? status(400)
  )
})
```

`signing_key` is the `secret{name: signing_key}` leaf, capability `SecretRead("signing_key")`,
granted at deploy like any other;
`import receipt_key from "secret:receipt_key" with { optional: "true" }` lets the deploy leave it
out, its reads then without a value. A secret not imported is an unknown name, like any other. The
value is set for the deployment (the flow's Settings, `holon secret set`), or once for the server
(`holon secret bind`), never in the flow.

- `secrets.rs`: the store (`SecretStore`), a deployment's values in memory (`Vault`, `Resolved`),
  the marks and the `Redactor`.
- `at_rest.rs`: the keyring every value is sealed under (`Keyring`, the `secrets` scope);
  `sealing.rs`: where its root key is kept (`KeySource`).
- `secret_sources.rs`: values kept elsewhere (`Reference`, `Sources`).
- `holon-core/src/taint.rs` and `crates/holon/src/taint.rs`: where a secret's value flows in the
  graph (`SecretFed`, `sent_secrets`).
- `secretcmd.rs`: `holon secret list|history|set|rollback|remove|bind|cap|bound|unbind|env`.

## 2. How it works

- **Stored sealed.** The secrets file (`--secrets`, `secrets.json`, owner-only) keeps each
  deployment's secrets as versions (`Stored`: version, time, source, sealed value), never a value
  in the clear. Each value is sealed as a line (`Keyring::seal_line`) with XChaCha20-Poly1305
  under the `secrets` scope's data key, the slug, name and version as associated data, so a sealed
  value moved elsewhere fails to open; and the file as a whole is a table sealed under the
  `server` scope (`at_rest::write_private`), as every table is (docs/L4s/at-rest-encryption-L4.md).
- **The root key** is kept apart from the data folder and wraps every data key in `keys.json`:
  `--root-key os` (the OS key store, the default on Windows and macOS), `env:VAR` (64 hex digits,
  never created; `--new-root-key` prints one) or `file:PATH` (owner-only, the default elsewhere,
  in `~/.holon/keys`, outside the data folder) (`KeySource::parse`). `keys.json` records a check
  of it, so a wrong root is told from a damaged file and the server refuses to start. The
  `secrets` data key is rotated with `holon key rotate secrets`: values sealed before still open.
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
- **A salt for each value.** The first time a value is read (set here, or fetched from where it
  is kept), the store draws it 32 random bytes and seals them as it seals a value, bound to the
  secret's name and version and the value's fingerprint (`SecretStore::salt_of`,
  `SecretsFile::salts`). A reference's value changes under one version, so a salt is a value's:
  one that comes back has its first salt again. It is the same after a restart, kept when its
  version is dropped or its secret removed, and gone with a deleted deployment. A deployment's
  current salts are in memory beside its values (`Resolved::salts`).
- **A commitment in each run's entry.** Beside the version and fingerprint of each secret a run
  was given (`secrets`), its entry holds what the secret is held to (`committed`,
  `recorder::Committed`): `local` or `reference`, and `secrets::commitment`, a SHA-256 over the
  secret's owner (the deployment, or the server), name and version, the value's salt and the
  value, each with its length. Whoever later holds the value and its salt computes it again;
  without the salt it says nothing of the value. `holon verify` holds an export to it as far as
  it can without the value: one value, one commitment in every run, and each mark of a secret
  set here reading the fingerprint its entry lists (`verify::listed_secrets`)
  ([secret verification](../../L4s/secret-verification-L4.md) §8.1, items F and H).
- **The server's own** are kept under `SERVER` (`*`) and read by a deployment with none of its
  own of that name; `--to` caps where one may go (`SecretStore::refuses` at deploy).
- **Redacted.** `Redactor::of(vault)` pairs each current and retired value, and its JSON-escaped
  form, with its mark, `‹secret:name v3 #a1b2c3d4›` (`mark`: the version and an 8-hex fingerprint
  keyed by a key derived from the `secrets` scope's first data key, `Keyring::fingerprint`, so it
  stays the same across a rotation). `Redactor::entry` marks everything a log entry
  carries (request, sourced values, state before, outputs, effects, reads, timeline, rounds,
  answer, error, url) and escapes a run's own look-alike of a mark (`write_marked`). Control-plane
  replies go through `Redactor::json`; exports carry the log's lines, already marked. Credentials
  a call answered to a request that held a secret (`ANSWER_FIELDS`: `access_token`, `token`, …)
  read `‹token:name #…›` (`Redactor::answers`), resume tokens `‹token:name›`.
- **Followed.** Core's taint walk follows a secret's value through the graph as literal or
  derived, cut by a one-way leaf (`sha256`, `hmac_sha256`, `secure_equals`), a Boolean or a call's
  answer. A state, collection, file, database or channel write of a secret or of what is derived
  from one is refused (`firing.rs`); a call to another flow's action carrying one is `refused`
  (`calls.rs`); a route answers with one only where `SecretAnswer("name")` is granted.
- **What is derived is marked, or withheld.** The walk names the leaves that make something of a
  secret (`SecretFed::makes`): each whose output may be computed from one and isn't only what it
  was handed, moved (`PASSES_LITERALLY`, less `sort` and `unique`, and less a leaf a secret
  reaches at an input that only chooses). Every derived value in a run is what such a leaf gave,
  or holds it. After a run, `secrets::born` reads what each maker gave off the timeline: a text
  of `MIN_SECRET_LEN` or more gets a hash, `derived_hash`: HMAC-SHA-256 under a key made of the
  salts of the secrets the run was given (`derived_key`), of the run's id and the value. The
  entry lists the hashes (`derived`), and `Redactor::born` writes `‹derived #` and the first
  eight digits wherever the run's record holds the value, a secret's own mark first where a
  value is both. That holds for the whole record only if the makers are all there (`Born::whole`):
  every deriving node followed and no deeper than `Timer::DEPTH` (`SecretFed::followed_to`: none
  inside a function `map` or `fold` applies, a module), the run kept a timeline, and each maker
  gave a text a mark stands for, one that reads the same written as JSON. Otherwise the record
  withholds what may be derived, by the graph alone, as before (`WITHHELD`:
  `FiredRun::withholds_output`, `withholds_effect`, `withholds_input`; a node's outputs left out
  by `without_secret_derived`), and the marks it does have still cover those values wherever
  else they turn up.
- **What a secret chooses.** A choice decided by what a secret reaches (`guard`, `coalesce`,
  `and`, `or`, which core runs lazily) runs only the branch it takes, and the run's timeline keeps
  the steps it took (`SecretFed::chosen`): a run's steps show which way a choice went, a secret's
  included, as the owner decided. Whoever approves a flow reads what it chooses by
  (`SecretFed::deciders`, each choice a secret decides, given that secret through
  `SecretFed::of_secret`), and reading a run's steps is a grant of its own. A run not marked
  whole keeps out what a secret chose to ask by: its read whose request a secret reaches
  (`SecretFed::fed`) keeps its collection, folder, database, query, `one` and `bytes` and withholds the rest
  (`taint::withhold_read`), and what it found, but a database's rows (`LoggedRead.withheld`,
  `FiredRun::withholds_read`); a request's headers are withheld whole, names and all. In any run,
  a claim on a key such a read chose reads `seen[‹withheld…›]` in the log and the 503's answer
  (`ClaimKey::shown`), and a read that failed by one says which kind of read failed, no more
  (`RunWaits.failed_at`). `sort` and `unique` give what a tainted input reaches derived (`ORDERS`).
  A route that reads its flow's own data by a secret (`SecretFed::reads_by_a_secret`: a
  collection's item, a folder's file) answers its caller something of it, found, waited on or
  failed, so it asks `SecretAnswer` (`answered_secrets`); a database's read doesn't, the database
  being where the approval says the secret goes.
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
  hashed, encoded or sliced isn't recognised by value. The taint walk marks or withholds what the
  graph derives; a value it can't follow (echoed back by an API and passed on) is caught only
  while it is literal.
- **A mark stands for a text of 6 characters or more.** A true or false, a number, bytes, a
  list, a record, a short text, and a text JSON escapes leave the run's record withheld whole,
  and so does a secret read inside a function applied to each item, inside a module, or deeper
  than the timeline records. Bytes could be marked and aren't yet.
- **A derived mark is a run's own.** The same value has another mark in the next run, so
  `verify` can't hold one run's record of a step that made a value to another's, as it does for
  a step that gave a hash.
- **Token fields by name.** An answered credential under another field name is followed by the
  graph but not marked as a token.
- **No run-time check on a database's parameters**: they go where the graph says, to the
  database the admin bound (secret destinations §8). A cap covers the server's secret, not what is
  answered to it, and names no deployment.
- **A database answers what its query makes of a secret.** Its rows, and its failing, are the
  database's, approved as where the secret goes: a route that answers them, or fails with them,
  may tell its caller something of the secret without `SecretAnswer` (the audit's F10, by the
  owner's choice).
- **A caller can tell which way a secret's choice went by time, and by a 503.** The steps
  that show it are a reader's, under a grant; a caller sees none, but how long the run took is
  not covered, nor a 503 answered when a key only one branch reads, clean itself, is held by
  another run.
- **A revoked Vault value works for up to `STALE_AFTER`**; a source read fails loudly, once, in
  the audit.
- **In memory in the clear** while the server runs (`Zeroizing`, wiped when dropped).
- **`--secrets :memory:`** keeps nothing across a restart: a waiting run's requests are sealed in
  a persisted deployments file under a key that is never kept, so after a restart (the secrets
  gone with it) the run can't be opened, and says so.

## 5. What the tests hold

- holon-core taint.rs: `a_leaf_makes_something_of_a_secret_unless_it_only_moves_what_it_was_handed`.
- secrets.rs: `a_value_computed_from_a_secret_has_one_mark_in_a_run_keyed_by_the_runs_salts`,
  `a_value_has_one_salt_drawn_once_sealed_and_kept_across_restarts`,
  `what_a_run_records_of_a_secret_is_where_it_is_kept_and_a_commitment_under_its_salt`,
  `a_store_keeps_values_sealed_and_survives_a_restart`,
  `a_missing_or_wrong_key_refuses_instead_of_starting_empty`, `versions_are_kept_and_old_values_still_redacted`,
  `a_mark_names_the_version_and_a_fingerprint_of_the_value`, `a_vault_value_follows_its_source_and_outlives_an_outage_only_briefly`,
  `a_rollback_restores_an_older_version_as_a_new_one_and_everything_is_audited`,
  `redaction_replaces_every_literal_occurrence`, `a_look_alike_of_a_mark_is_escaped_and_reads_back_as_text`,
  `with_nothing_to_mark_a_text_without_a_mark_sign_stays_as_it_is`,
  `the_servers_secret_is_read_where_a_deployment_has_none_and_keeps_its_cap`.
- sealing.rs: `a_sealed_value_opens_only_where_it_was_sealed`; secret_sources.rs:
  `references_parse_and_only_holon_variables_can_be_named`, `a_dotenv_file_reads_like_the_usual_tools`.
- tests.rs: `a_runs_record_reads_the_same_whatever_its_secret` (one flow run with two secrets:
  the two records read the same but for marks, hashes and commitments, in each way a flow
  computes from a secret), `a_secret_reaches_its_runs_and_nothing_else`, `a_secret_computed_into_something_else_stays_out_of_the_log`,
  `what_is_computed_from_a_secret_is_neither_kept_nor_logged`, `a_secret_goes_only_where_its_approval_shows_it_going`,
  `the_servers_secret_goes_only_where_an_admin_said_it_may`, `a_token_a_call_answers_with_a_secret_is_kept_out_of_the_log_and_out_of_state`,
  `a_route_answers_with_a_secret_only_where_a_person_approved_it`, `a_run_shows_which_secret_value_it_had_and_still_checks_after_it_rotates`,
  `a_secret_stays_out_of_files_and_of_what_the_log_keeps_of_them`, `a_secret_is_never_sent_to_another_flow`.
- **Samples**: `guarded-api` (sources, versions, rollback), `sign-payload` (verify with a secret),
  `stripe-to-slack`, `echoed-key`, `client-credentials`, `license-check` (where each goes),
  `browser-key` (`SecretAnswer`), `shared-key` (the server's, capped), `basic-auth` (a value
  computed from a secret, marked and run again; one no mark stands for, withheld). All deploy and answer in
  `sweep.e2e.ts`; `shared-key` (its refusal) and `guarded-api` (versions in Settings) have tests in
  `samples.e2e.ts`, as does an env secret picked from `HOLON_SECRET_…` names with its dotenv file.
  What the approval says beside each secret is `touches.rs`'s `a_secret_s_row_says_where_it_goes`,
  and the canvas's marks `commands.rs`'s, on these samples' texts.
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
