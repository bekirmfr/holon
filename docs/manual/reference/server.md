# Server configuration

`holon serve` is the whole platform: the control plane (the editor, accounts, deployments, the
API the editor and `holon mcp` use) on one address, and one small HTTP server per deployed server
flow, on the flow's own port. This page covers what it keeps, its policy, accounts, secrets and
how it answers. Its options are in the [command line reference](cli.md#holon-serve).

## Where it keeps things

Everything is under one folder, `~/.holon` (`%USERPROFILE%\.holon` on Windows), or `HOLON_HOME`:

| Path | |
|---|---|
| `bin/` | the program |
| `data/` | the server's state: **the one folder to back up** |
| `keys/` | the secrets key, where there is no OS key store |
| `run/` | a file per running server, so commands on this machine find it and sign in |
| `tokens.json` | API tokens `holon mcp setup` keeps for servers elsewhere, readable only by you |
| `mcp-hosts.toml` | agent hosts you added to `holon mcp hosts` |

In the data folder (`--data`, `HOLON_DATA`):

| File | Holds | Option |
|---|---|---|
| `store/` | every unit, by hash | `--store` |
| `deployments.json` | the deployments, their grants, sessions and parked runs | `--deployments` |
| `deployments.logs/` | each deployment's run log, `<slug>.jsonl` | |
| `flows.json` | saved flows | `--flows` |
| `accounts.json` | accounts, sessions, invites and API tokens (as hashes) | `--accounts` |
| `secrets.json` | sealed secrets; `secrets.audit.jsonl` beside it, who changed what (never values) | `--secrets` |
| `folders.json` | the named folders bound | `--folders` |
| `databases.json` | the named databases bound | `--databases` |
| `holon.toml` | the policy | `--policy` |
| `host.key` | the key resume tokens are signed with | `--host-key` |

Files with secrets or tokens in them are readable only by the server's user. The server refuses
to start when one of them can't be read, rather than starting without it.

To move a server, stop it and copy the data folder; the secrets key goes with it only if it is in
a file (see [The secrets key](#the-secrets-key)).

## The policy

`holon.toml` in the data folder (or `--policy PATH`) says what this server may ever grant, and
what it grants without asking. A missing or empty file means the defaults. It is read at start;
`POST /permissions/reload` reads it again (a file that doesn't parse then changes nothing). The
editor shows it but doesn't edit it.

```toml
[limits]
request_timeout_ms = 20000

[ceilings]
max_requests = 200

[grants]
auto = ["StateRead(*)", "HttpRequest(\"hooks.slack.com\")"]
allow_plain_http = true
```

- **`[limits]`**: the defaults a deployment gets when it asks for nothing more.
- **`[ceilings]`**: the most a deployment may be granted. A default can't be above its ceiling.
- **`[grants]`**:
  - `auto`: items granted without asking, each exactly (`HttpRequest("hooks.slack.com")`) or a
    whole kind (`StateRead(*)`). Names are in [Permissions](permissions.md).
  - `allow_plain_http`: whether a deployment may ever be granted plain `http://` calls (default
    `false`).
  - `allow_private_addresses`: whether it may ever reach this machine and its network (default
    `false`).

An unknown section or key is a mistake, and so is a limit name the server doesn't know. The limits,
their defaults and their ceilings are in [Permissions](permissions.md#limits).

At deploy, each item of the manifest is either **refused** (above a ceiling, or plain HTTP or
private addresses the policy doesn't allow), **granted** without asking (in `auto`, a limit at or
below its default, or already in this deployment's grant) or **needs a person**. A grant is clamped
to the current ceilings whenever the server loads.

## Accounts and roles

Everyone signs in; there is no trust for requests from this machine. A new server prints a
setup code (`XXXX-XXXX`, or `HOLON_SETUP_CODE`); whoever enters it in the editor is the first
admin. Until then the server answers everything but setting up with 403.

| Role | May |
|---|---|
| viewer | read: flows, deployments, logs |
| editor | everything a viewer may, and save, deploy and approve, run, pause, stop, delete, change grants |
| admin | everything, and set secrets, bind folders and databases, manage accounts, tokens and sign-in |
| agent | read, check, save flows (never replacing someone else's), deploy what needs no approval, call routes |

- **Passwords** are at least ten characters, kept as Argon2id hashes. Five wrong in a row lock the
  name for a minute.
- **Sessions** last seven days. **Invite** and **reset** links work once, within seven days.
- The server never lets the last enabled admin be removed, disabled or demoted.
- **API tokens** (admin page) act with the role they were made with, as `token:<label>`, sent as
  `Authorization: Bearer <token>`. They are shown once and kept only as hashes; revoking one stops
  it at once.
- Commands on this machine (`holon export`, `holon folder`, `holon mcp`) sign in through the run
  file in `~/.holon/run/`, as `local:<your user>`, while that server runs.
- A request with `X-Holon-Role: agent` from an editor or an admin is treated as the agent role:
  `holon mcp` sends it, so an agent acting through your sign-in still can't approve. It never
  raises a role: a viewer who sends it stays a viewer, since an agent may do more than read.

### Signing in through an identity provider

An admin can turn on OpenID Connect on the admin page, with the provider's issuer URL and a
client id (a public client; the editor signs in with the authorization code flow and PKCE). A
person signs in through the provider when an account here has their email. Accounts are still
made here, with a role; the provider only proves who is signing in.

## Secrets

Secrets are set per deployment, by an admin, in the editor (**⚙ → secrets**); see
[Deploying](../guides/deploying.md#secrets). The server keeps them in `secrets.json`, each value
sealed with AES-256-GCM, the deployment, name and version bound in.

### The secrets key

`--secrets-key` says where the key is:

| Value | |
|---|---|
| `os` | the OS key store (the default on Windows and macOS) |
| `file:PATH` | a file of 64 hex digits, readable only by the server's user, made if missing (the default on Linux: `~/.holon/keys/`) |
| `env:VAR` | an environment variable holding 64 hex digits; `holon serve --new-secrets-key` prints one |

Without the right key the server doesn't start. Back the key up apart from the data: one without
the other opens nothing.

### Secrets from elsewhere

A secret's source can be `env` (`HOLON_SECRET_<NAME>` in the server's environment, or in the
`--dotenv` file; the real environment wins) or `vault` (`mount/path#field`, from HashiCorp Vault
KV v2 at `VAULT_ADDR`, with `VAULT_TOKEN` and `VAULT_NAMESPACE`). Both are read again every
minute; a value that can't be read is kept for five minutes at most.

## Deployed flows' servers

Each deployed server flow listens on its own port, on `--data-host` (127.0.0.1 unless told
otherwise). It speaks HTTP/1.1, without TLS: HTTPS ends at your ingress or proxy in front of it
([A public HTTPS address](../guides/public-https.md)). `holon serve` warns as it starts when the
server or these ports listen beyond loopback. It has no sign-in: a flow guards its own routes.

| Case | Answer |
|---|---|
| no route for the path | 404 `{"error":"no route for this path"}` |
| a route, but not that method | 405, with `Allow` |
| a body that isn't UTF-8 | 400 |
| a body over 10 MiB | 413 |
| a body not sent within 30 seconds | 408 |
| paused or stopped | 503 |
| more runs waiting at `await` than allowed | 503 |
| a resume token that isn't valid | 404 |
| the route's answer has no value | its `missing` status (404 unless set) |

A browser page from another site may call the routes only if the flow lists its origin in
`browsers` (or the deploy settings' **web pages from**).

Every request is a run in the log, even one no route took.

## The run log

Each deployment's log is `deployments.logs/<slug>.jsonl`: one JSON entry per run, each with the
hash of the one before. It keeps the newest `--keep-runs` runs (500 by default). An entry holds the
run's request, what it read (clock, randomness, state, files, each outbound response), its
outputs, the effects it performed, its answer (up to 64 KiB of the body), its timings, the unit
and the grant it ran under, and who started it. Secrets and resume tokens are replaced by
`‹secret:name›` and `‹token:name›` before anything is written.

`--export-heads URL` sends each new head to a witness every five seconds, with
`HOLON_EXPORT_HEADS_TOKEN` as a bearer token; heads that can't be sent wait in
`deployments.heads/`. See [Checking runs](../guides/checking-runs.md#witnesses).

## Limits a run has

Every run has a time budget (25 seconds for a request, 60 for a clock run or a new file, by
default), a number
of outbound requests, and size limits on what it sends and receives. A run past its budget fails,
and performs nothing. The defaults and ceilings are in [Permissions](permissions.md#limits).
