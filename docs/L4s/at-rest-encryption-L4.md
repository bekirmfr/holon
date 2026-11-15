# Encryption at rest: every byte a flow leaves on disk, sealed under keys the server never stores beside it (L4)

> **Dev-phase document: not for the launch repository.** It describes open limits of the design
> and work in progress. It is kept here while Holon is being built, and is left out when the
> public platform repository is made, which starts from a fresh history (owner, 2026-11-15).
<!-- holon:dev-phase-only -->

**Status:** built (2026-11-14 and 15), in the steps of §9; §10 stays open. The design below was
written on 2026-11-14, grounded at `origin/main` `7077129a` (branch `launch-fixes-4`, one commit
ahead, which touches none of the files below): every claim about the code names its file and line
as they were at that commit, and "today" is the server before it. The code cites this note by
section; what the server does now is in docs/engineering/capabilities/storage.md and
docs/engineering/themes/security.md.

Today the server seals two things: each secret value (`secrets.rs`, AES-256-GCM under one master
key, `sealing.rs`) and the part of a waiting run that may hold one (`ParkedRun::sealed_for_disk`,
`durable.rs:746`). Everything else a flow leaves in the data folder is plain JSON or plain bytes in
owner-only files: the run log, its sealed segments, events, console lines, state and collections,
file texts, deployments with their flow texts, mail and queue spools, accounts. Anyone who can read
the folder, a backup of it or a disk image, reads every request, answer and state value. This note
makes it:

> **Every file the server writes under its data folder or its backup folder holds no run data in
> clear.** Each is sealed with a data key; data keys are kept in the data folder only wrapped by a
> root key; the root key is kept where the secrets key is kept today (the OS key store, an
> environment variable, a key file beside the data folder, later a KMS), never in the folder.
> A deployment has keys of its own, so forgetting them makes what it wrote unreadable on this server
> and in the backup folder; copies of the whole data folder made earlier are reached only when the
> old root goes too (§2.4). Exports, verify and the witness are unchanged: they see plaintext
> lines, and the chain hashes plaintext.

## 1. What is on disk today

From the data folder's definition (`main.rs:920-965`: `--data`, then `HOLON_DATA`, then
`~/.holon/data`; each default named by `inside(…)`, `main.rs:936-947`), and the files derived
beside `deployments.json` (`Place::beside`, `paths.rs:132`).

| Data | Path | Written by | Shape | Holds run data |
|---|---|---|---|---|
| Run log, hot file | `deployments.logs/<slug>.jsonl` (`recorder::file_for`, `recorder.rs:1297`) | `Log::push_made` `recorder.rs:1626` → `keep_line` `:1679`, raw append, kept handle | line per run | **yes**: inputs, answers, reads, state keys |
| Run log, sealed segments | `deployments.logs/<slug>.sealed/<first>-<last>.jsonl.gz` (`sealed_dir` `:970`) | `write_segment` `:984` (gzip, `write_aside`) | whole file | **yes** |
| Run log, cut-back / restart rewrite | same hot file | `cut_back` `:1855` (`write_private` `:1917`), `rewrite_file` `:1485` (`:1497`), `CutPlan::make` `:686` (`write_aside` `:718`) | whole file, lines copied byte for byte | **yes** |
| Events (timeline) | `deployments.logs/<slug>.events.jsonl` (`events.rs:101`) | `Events::keep_line` `events.rs:168` | line, own chain | **yes** (what/by/detail) |
| Console lines (`log.info`, …) | `deployments.logs/<slug>.jsonl`, rolled `<slug>.1.jsonl` (`logs.rs:338`) | `logs::append` `logs.rs:343`, plain `OpenOptions` | line | **yes** — and see §1.1 |
| Removed deployment's records | `deployments.logs/removed/<slug>-runs-<ms>.json.gz` (`deploy.rs:1002`) | `keep_records` `deploy.rs:1009` (`write_private` `:1030`) | whole (an export) | **yes** |
| Backup batches, archives | `<backup>/<slug>/holon-archive-<first>-<last>.jsonl[.gz]` (named by `storage::batch_name`, `storage.rs:77`) | `keep_aside` `recorder.rs:729` (raw line bytes), `keep_segments_aside` `:766` (segments' lines), `archive_one` `storage.rs:1155` (reads the batch as text, `:1156`, hashes its last line, `:1159`; `write_atomically`) | lines, then whole (an export) | **yes** |
| Deployments table | `deployments.json` | `persist_deployments` `persist.rs:22` (`write_private` `:69`) | whole | **yes**: flow texts (`document`, `versions_kept`), grants, waiting runs' unsealed parts |
| Waiting runs' secret part | inside `deployments.json` | `sealed_for_disk` `durable.rs:746`, context `parked_context` `durable.rs:589` | AEAD blob | sealed today (secrets key) |
| State head | `deployments.state/<slug>` (`persist.rs:80`) | `StateCell::write_head` `claims.rs:78` | 64 hex | no (a hash) |
| State values, collections, snapshots | `store/values/<hash>`, `store/values/packs/<id>.pack` | `FileStore::put_value` `holon-host/src/lib.rs:628`; `Packs::write` `holon-host/src/packs.rs:222` | content-addressed, name = blake3 of plaintext | **yes** |
| Units (compiled flows) | `store/<hash>` | `FileStore::put` `holon-host/src/lib.rs:614` | content-addressed | **yes** (flow code, frozen params) |
| File texts kept for the log | `store/blobs/<id>`, `store/blobs/chunks/<hash>` | `blobs.rs:183`, `:188` | content-addressed | **yes** |
| Saved flows, proposals | `flows.json`, `flows.proposals.json` | `persist_flows` `persist.rs:300` (`write_atomically` `:305`), `proposals.rs:96` | whole | **yes** (flow texts) |
| Mail / queue spools | `mail-spool/<slug>.<run>.<n>.mail`, `queue-spool/…msg` | `Mails::spool` `mails.rs:588` (`:615`), `queues.rs:431` | whole per message | **yes** (bodies) |
| Seen ids, arrivals | `deployments.{queue,mail,feed}-seen/<slug>/…`, `deployments.arrivals/<slug>.json` | `dequeue.rs:105`, `inbox.rs:73`, `subscriptions.rs:217`, `arrivals.rs:127` | whole | ids, file names |
| Bindings | `folders.json`, `databases.json`, `mails.json`, `queues.json`, `feeds.json` | `write_private` at `folders.rs:390`, `databases.rs:352`, `mails.rs:521`, `queues.rs:359`, `feeds.rs:603` | whole | addresses, paths, references to secrets |
| Accounts, access | `accounts.json`, `access.json`, `access.versions.jsonl` | `accounts.rs:577`, `bindings.rs:606`/`:609` | whole / line | sessions (token hashes), names, emails |
| Activity | `deployments.activity.jsonl` | `activity.rs:96` | line | who did what |
| Secrets, audit | `secrets.json`, `secrets.audit.jsonl` | `secrets.rs:770`, `:215` | whole / line | values sealed; audit names only |
| Resume-token key | `host.key` | `durable::load_host_key` `durable.rs:96` (`:116`) | 64 hex | **a key in clear** |
| Witness queue, receipts | `deployments.heads/pending.jsonl`, `receipts.jsonl` | `heads.rs:233`/`:256`, `:335` | line | no (hashes, batch ids) |
| Policy | `holon.toml` | `storage.rs:1280` | whole | no (hand-edited config) |
| Clean-stop mark | `deployments.stopped` | `stopping.rs:138` | whole | no |

Not in the folder, and out of scope (§8): bound folders (user paths, `folders.rs:921-945`), SQLite
files (refused inside the data folder, `databases.rs:307`), and external queues, mail servers,
databases.

### 1.1 Found on the way: two writers share one file name

`logs::Logs::beside(&deployments_at, …)` (`main.rs:1071`, `logs.rs:157-158`) puts console lines at
`<deployments>.logs/<slug>.jsonl` (`logs.rs:338-340`), and the run log is
`<deployments>.logs/<slug>.jsonl` too (`recorder.rs:1297-1299`), from the same `deployments_at`.
Read from the code (an independent review read it the same way), not yet run; when a flow writes
console lines:

- they land in the run log's file, where `lines_of` (`recorder.rs:933`) parses them as `LogEntry`
  (`#[serde(default)]`), and a cut-back or verify meets lines without a chain;
- `logs::append` sizes the file with the run log's bytes in it and rolls early: it **renames** the
  file (`logs.rs:348-352`) from under the recorder's kept handle, so runs go on landing in
  `<slug>.1.jsonl`, and a restart's `open_retaining` reads a file of console lines;
- the storage census sizes the same file (`storage.rs:469`).

Step 0 confirms it with a test and moves console lines to `<slug>.console.jsonl`. It must be
settled first: the log codec (§5.1) would otherwise reject the other writer's lines.

### 1.2 Also found: permissions are not uniform

`receipts.jsonl` (`heads.rs:335`) and console lines (`logs.rs:354`) are opened with plain
`OpenOptions`; `holon_host::write_atomically` (`lib.rs:564`) and `append_private` (`sealing.rs:306`)
set 0o600 on Unix but no owner-only DACL on Windows, so on Windows the store, packs, blobs, flows,
backups and appended lines inherit the folder's ACL. Encryption makes this matter less for content,
not for the root key's file, so step 0 makes every writer owner-only.

## 2. Decisions

### 2.1 Encrypt in the server, not ask for disk encryption

BitLocker and LUKS protect a stolen disk; they don't protect a backup, a copied folder, a support
bundle, a cloud snapshot or another account on the machine. A payment-ops buyer will ask "who
holds the keys?", and "the disk's" is not an answer they can audit. Disk encryption stays
recommended (`docs/manual/reference/server.md`), not relied on.

### 2.2 The chain hashes plaintext; files hold ciphertext

`line_hash` (`recorder.rs:61`) is blake3 of a line's plaintext bytes; `prev` names it; the witness
is sent heads (`Log::export`, `heads.rs`); `holon verify` re-hashes export lines (`verify.rs:440`,
events `:922`). All of it keeps working if the chain stays over plaintext and only the file holds
ciphertext:

- the witness learns nothing more than today (a hash), and keeps comparing heads across a key
  rotation;
- an export is plaintext (§6), so `holon verify` and the offline pitch are unchanged;
- a rotation never rewrites the chain.

The alternative, chaining ciphertext, would let anyone check the chain without the key but break it
on every rotation and make every export carry ciphertext no verifier can read. Rejected.

### 2.3 XChaCha20-Poly1305 for data, not AES-256-GCM

`Sealer::seal` (`sealing.rs:63`) draws a random 96-bit nonce per value. That is right for secrets
(a few thousand values per key), not for a run log: at lazy's measured 22,388 runs a second
(`docs/engineering/themes/performance.md:511`) one key passes the 2^32 random-nonce bound NIST
SP 800-38D sets for GCM in about 53 hours. XChaCha20-Poly1305 takes a 192-bit random nonce, for
which collisions don't bound use. It is the RustCrypto `chacha20poly1305` crate, the family of the
`aes-gcm` 0.10 already used (`crates/holon/Cargo.toml:15`); in software it runs about 1–2 GB/s per
core, a few µs for a log line (§7). One AEAD everywhere: secrets move to it too (dev phase, no
compatibility, memory `dev-phase-no-compat`).

### 2.4 Envelope keys: one root, data keys per scope

```
root key (32 bytes)            where --root-key says: os | env:VAR | file:PATH   (later kms:…)
  │                            never in the data folder
  └─ wraps, in <data>/keys.json:
       server   v1, v2, …      deployments.json, flows, proposals, bindings, accounts, access,
                               activity, store (units, values, packs, blobs), spools, host.key
       secrets  v1, …          secrets.json values
       names    v1 only        the store's file names (§2.6), never rotated
       d:<slug> v1, v2, …      that deployment's run log, segments, events, console lines,
                               removed records, backups/archives, waiting runs' sealed part,
                               seen ids, arrivals
```

- **Root rotation** re-wraps the few data keys in `keys.json`; no data file is touched.
- **Data-key rotation** adds a version; new writes use it, every record names the version that
  sealed it, old versions stay to read old records until nothing names them.
- **Forgetting `d:<slug>`** removes its wrapped keys from `keys.json`, so every file that
  deployment wrote is unreadable to this server, in the data folder, the backup folder and the
  removed-records folder. It does **not** reach a copy of the whole data folder taken before (a
  snapshot, an image): that copy holds the old `keys.json`, which the root still opens. So
  forgetting also **rotates the root** (re-wraps the remaining scopes under a new root and removes
  the old one from its source): an earlier copy then needs a root that no longer exists. That
  holds only if nobody kept the old root; an operator who backs up the root (as they should, §8)
  keeps the earlier copy readable, and the manual says so. Real per-scope shredding against every
  copy needs a key that can't be restored, which is `kms:` with a KMS key per deployment,
  destroyed on forget (§10).
- **The store stays one scope** (`server`): it is content-addressed and shared across deployments
  (a value two deployments hold is one file, `sweep.rs` counts it `shared`). A deployment's values
  leave it by the sweep once nothing names them (`sweep.rs:1-25`); §8 says what forgetting does
  and doesn't reach there.

### 2.5 What stays in clear, on purpose

Hashes and names that are already public or carry no run data: the state head (`claims.rs:78`, a
hash the export carries anyway), the witness queue and receipts (heads, batch ids), `holon.toml`
(hand-edited), the clean-stop mark, file names (a slug, a seq range, a run id), file sizes and
modification times. A note in `docs/manual/reference/server.md` says what an attacker with the
folder learns: which deployments exist, how many runs, when, and how large.

### 2.6 Content-addressed names are keyed

Store files are named by the blake3 hash of their plaintext (`FileStore::value_path`,
`holon-host/src/lib.rs:542`; the sweep reads names back as hashes, `sweep.rs:250-255`). Left so,
anyone with the folder can test a guess (`blake3(encode("approved"))`) against the names, which
leaks small state values even under encryption. Names become `keyed_blake3(names_key, hash)` in
64 lowercase hex, so everything that recognises a store file by its name's shape keeps doing so
(the storage census, `count_dir(&dir, |n| n.len() == 64, …)`, `storage.rs:549`; blob ids,
`is_id`, `blobs.rs:80-83`). Everything inside the server still speaks hashes.

`names_key` comes from a scope of its own, `names`, which is **never rotated**: a name must stay
what it was, or a rotation would lose every value stored before it and store a value twice.
Rotating it would mean renaming the whole store; not offered. A root rotation still re-wraps it.

### 2.7 The CLI never holds a key, but two offline commands may

`holon secret` is a control-plane client (`secretcmd.rs:63`) and stays one. Exports come from the
server decrypted (§6). Two commands read the folder directly and will need the root key:
`holon migrate` (`check_copy`, `migrate.rs:250`) and `holon verify` of a backup archive or a
removed deployment's records. Both take `--root-key` and default to the same source the server uses.

## 3. Keys

### 3.1 The root key, where the secrets key is kept now

`KeySource` (`sealing.rs:127-213`) becomes the root key's source with no change of kinds: `Os`
(Credential Manager, Keychain, through `keyring` 3, `Cargo.toml:91,100`), `Env(VAR)`, `File(PATH)`;
the default is `Os` on Windows and macOS, `~/.holon/keys/<id>.key` elsewhere (`KeySource::parse`,
`sealing.rs:140-158`). What changes:

- **Its subject.** The OS entry is `holon` / `root-key-<id>`, the file `<id>.key`, where `<id>` is
  `keys.json`'s `key_id` (today `secrets.json`'s, `sealing::key_id`, `sealing.rs:94`). The flag is
  `--root-key` (today `--secrets-key`, `main.rs:809-814`), and `holon serve --new-root-key` prints
  one (today `--new-secrets-key`, `main.rs:838-841`).
- **A recorded random id.** A new `keys.json` takes 16 random hex digits, not the path's hash
  (`path_key_id`, `sealing.rs:104`). This also settles the disagreement the key-handling survey
  found: `docs/L4s/holon-home-L4.md:87` says "a new store takes a random id", while the code
  records the path hash (`secrets.rs:325` via `key_id`).
- **Its check.** `keys.json` records `root_check = keyed_blake3(root, "holon root key check")[..16]`
  (as `Sealer::new`, `sealing.rs:41`), compared before anything opens: a wrong key refuses to start
  (`refuse_to_start`, `main.rs:1021-1025`), as a wrong secrets key does now (`secrets.rs:367`).
- **Purge.** `uninstall --purge` forgets it (`selfmanage.rs:244-250`, today with the default source
  only: it gains the `--root-key` the server was started with, read from `holon.toml`'s
  `[keys] root = "…"` when set there).

`SecretStore`'s own key handling (`secrets.rs:302-380`) goes: it is given its `secrets` data key by
the keyring. The plain-values upgrade (`secrets.rs:327-345`) and format 2 (`secrets.rs:351-354`)
are deleted with it (dev phase).

### 3.2 `keys.json`

Written by `write_private`, beside the files it unlocks (it is useless without the root):

```json
{
  "format": 1,
  "key_id": "9f2c41d07ab3e655",
  "root_check": "1c0e8a7d55b2f940",
  "aead": "xchacha20poly1305",
  "scopes": {
    "server":  { "current": 2, "versions": { "1": { "wrapped": "<b64>", "made_ms": 1763100000000 },
                                             "2": { "wrapped": "<b64>", "made_ms": 1765700000000 } } },
    "secrets": { "current": 1, "versions": { "1": { "wrapped": "<b64>", "made_ms": … } } },
    "names":   { "current": 1, "versions": { "1": { "wrapped": "<b64>", "made_ms": … } } },
    "d:shop":  { "current": 1, "versions": { "1": { "wrapped": "<b64>", "made_ms": … } } }
  },
  "forgotten": { "d:old-shop": 1765000000000 }
}
```

- `wrapped` = 24-byte nonce ‖ XChaCha20-Poly1305(root, data key), associated data
  `holon-key/v1\0<key_id>\0<scope>\0<version>`, so a wrapped key moved to another scope, version or
  folder fails to open.
- From each data key `k`, by `blake3::derive_key`, the server derives what it uses and never
  stores: `enc = derive("holon at-rest v1 seal", k)`; from the `names` scope's single version,
  `names = derive("holon at-rest v1 names", k)` (§2.6); from `secrets`, `prints = derive("holon
  secrets fingerprints", k)` (today from the master key, `sealing.rs:42`).
- `host.key` (`durable.rs:96-116`) moves in as a `server`-scope record (§4), so the resume-token
  key is no longer a file in clear.
- A missing `keys.json` with no data files makes a new keyring; a missing one with sealed files
  refuses to start (as `secrets.rs:361-363`).
- `forgotten` keeps the names of shredded scopes, so a file sealed under one is said "forgotten
  (deployment removed on …)", not "damaged".

### 3.3 In memory

`at_rest::Keyring` holds, for each scope and version, the unwrapped `enc` key as
`Zeroizing<[u8; 32]>` and a ready `XChaCha20Poly1305`; it is shared (`Arc`) where `Sealer` is now
(`secrets.rs:68-71`, `persist.rs:56`, `main.rs:1085`). The root is dropped (wiped) once the
keyring is open; a root rotation or a new scope reads it again from its source.

## 4. One record format

`crates/holon/src/at_rest.rs` (new) is the only code that seals or opens bytes for the disk.

**A whole file** (tables, segments, spools, store files, exports kept on disk):

```
"HLA1" ‖ u8 scope_len ‖ scope ‖ u32 version (LE) ‖ 24-byte nonce ‖ ciphertext+tag
AD = "HLA1" ‖ scope ‖ version ‖ "\0" ‖ role
```

`role` names what the file is, so a sealed file renamed to another role fails to open: `table:
deployments`, `table: flows`, `segment: <slug>`, `spool: mail`, `store: value`, `export:
<slug>`… (a `const` per call site, listed in `at_rest::Role`).

**A line** (run log, events, console lines, activity, access history, secrets audit):

```
"a1." ‖ version (decimal) ‖ "." ‖ base64url(24-byte nonce ‖ ciphertext+tag) ‖ "\n"
AD = "a1" ‖ scope ‖ version ‖ "\0" ‖ role
```

Base64 keeps lines splittable on `\n` with no escaping, so every reader that splits a file into lines
(`lines_of` `recorder.rs:933`, `each_line` `:1209`, `read_if` `:1061`, `Events::lines`
`events.rs:209`) keeps its shape: it decodes each line it reads. A line's AD doesn't name its seq,
so a cut-back can copy ciphertext lines byte for byte (`rewrite_file` `recorder.rs:1491-1494`,
`CutPlan::make`), as it copies plaintext ones now. What that leaves to the chain: lines moved,
repeated or swapped within one deployment's log open, and are caught where they are now, by
`prev` (`open_retaining`'s break check, `recorder.rs:1418`, and `holon verify`). Lines dropped
from the end are not caught locally, as today; only the witness sees a head go back.

**Not protected: rolling back a whole file.** A file's AD names its scope, version and role, not
its age, so someone who can write the folder can put back an older sealed `deployments.json`,
`accounts.json`, `access.json` or `keys.json` (which would bring back a forgotten scope), and it
opens. The chain covers logs only. That is a non-goal here (the threat is reading, not writing);
a counter kept outside the folder, or sent to the witness with the heads, would close it (§10).

**API** (all take a `&Keyring` and a `Scope`):

- `seal_file(scope, role, &[u8]) -> Vec<u8>` / `open_file(role, &[u8]) -> Result<Zeroizing<Vec<u8>>, Unreadable>`
- `seal_line(scope, role, &str) -> String` / `open_line(role, &[u8]) -> Result<String, Unreadable>`
- `write_private(path, scope, role, &[u8])` and `write_atomically(…)`: seal then the existing
  helpers (`sealing.rs:317`, `holon-host/src/lib.rs:564`).
- `Unreadable` says which: not sealed (a plain file where a sealed one belongs), damaged, a key
  version this keyring lacks, a forgotten scope, another folder's key.

A plain file where a sealed one belongs is refused, never read: there is no mixed mode and no
"plaintext fallback", or an attacker who can write the folder could plant one.

## 5. Each store

### 5.1 The run log

All in `recorder.rs`. The log gains `seal: Option<at_rest::LineSeal>` (scope `d:<slug>`, role
`log`), set where the log is opened (`Log::open_retaining`, `:1322`).

- **Writing.** `push_made` (`:1626`) builds the plaintext line as now (`Made::line` `:899`, or
  `serde_json`), hashes it for the chain (`line_hash`, `:1653`), and gives `keep_line` (`:1679`) the
  sealed line. Sealing happens under the log's lock, because seq and prev are only known there
  (`Made`'s doc, `:862-866`); the cost is in §7.
- **`LineAt.len`** (`:918`) becomes the length on disk (sealed), which is what `rewrite_file`,
  `cut_back` and `CutPlan::make` slice the file by (`:1491`). `entry_bytes` and `entry_lines`
  (`:1643-1644`) stay plaintext lengths: they count memory, not disk.
- **Reading.** One decoder, `LineSeal::open`, at the four places a file's bytes become text:
  `lines_of` (`:933`), `each_line` (`:1209`), `read_if` (`:1061`, before `keep(line)`, which
  matches plaintext), `Log::lines` (`:1977`). `cut_back`'s re-index of a changed file (`:1863-1868`)
  decodes to find each line's seq and prev. A line that doesn't open counts as a broken line
  (`open_retaining`'s break check, `:1418`), as a line that doesn't parse does now.
- **Segments.** `write_segment` (`:984`) gzips the plaintext lines (sealed lines don't compress),
  then seals the gzip as a whole file (role `segment: <slug>`); `read_segment` (`:975`) opens, then
  gunzips. A cut decodes the hot lines it moves into a segment.
- **Everything built on the readers follows**: `summary.rs:223` (re-computes the head from
  plaintext), `sweep.rs:213` (hex names in plaintext), `folders.rs:1912`, `channels.rs:155`,
  `changes.rs:68`, `deploy.rs:1549`/`:1675`, spool recovery `main.rs:1095-1097`.
- **Startup.** `open_retaining` runs before any run, with the keyring already open (§9 step 1
  orders it in `main.rs`).

### 5.2 Events, console lines, activity, access history, secrets audit

The same line format: `Events::keep_line` (`events.rs:168`) and `Events::lines` (`:209`), its chain
over plaintext (`:125`, `:158`); `logs::append` (`logs.rs:343`) and `Logs::read` (`:197`, scope
`d:<slug>`); `activity.rs:96`/`:107` and `bindings.rs:606`/`:627` (scope `server`);
`secrets.rs:206-215` (scope `secrets`).

### 5.3 Tables

`deployments.json` (`persist.rs:69`, read `:106`), `flows.json` (`:305`, read `:318`), proposals
(`proposals.rs:96`), the five bindings files, `accounts.json` (`accounts.rs:577`, read `:538`),
`access.json` (`bindings.rs:609`, read `:527`), `secrets.json` (`secrets.rs:770`, read `:307`), seen
ids and arrivals: each `write_private`/`write_atomically` becomes `at_rest::write_private(path,
scope, role, bytes)`, each `fs::read` becomes `at_rest::read(path, role)`. `secrets.json` keeps its
per-value sealing too (each value under `secrets`, context `holon-secret/v3\0slug\0name\0version`,
`sealing::context`, `sealing.rs:87`), so a value read into memory to resolve a run is opened alone.

Waiting runs keep their sealed part (`durable.rs:746-777`), now under `d:<slug>`, context
`parked_context` (`durable.rs:589`); the rest of the run is sealed with `deployments.json`. A run
written without a sealer today (`persist.rs:59`, `sealer.as_deref().map_or_else(|| p.clone(), |s|
p.sealed_for_disk(slug, s))`) can't be any more: the keyring is always there.

### 5.4 The store (`holon-host`)

`holon-host` doesn't depend on `holon`, so it gets a trait, implemented by `at_rest`:

```rust
pub trait AtRest: Send + Sync {
    fn seal(&self, role: &'static str, plain: &[u8]) -> Vec<u8>;
    fn open(&self, role: &'static str, sealed: &[u8]) -> io::Result<Zeroizing<Vec<u8>>>;
    fn name(&self, hash: &Hash) -> String; // keyed, §2.6
}
```

`FileStore::new(root, Arc<dyn AtRest>)`: given at construction, not set after, because opening
the store already reads the pack indexes (`Packs::shared`, `holon-host/src/lib.rs:526`), and the
server opens it at `main.rs:997`, before the secrets key is even parsed (`:1027`). So the keyring
opens before the store (§9 step 1). Then:

- `put` / `get` (`lib.rs:614-626`): seal the encoding, open before `decode_verified` (which keeps
  checking the hash of the plaintext).
- `put_value` / `get_value` (`:627-652`), `path` / `value_path` (`:534-545`): keyed names.
- **Packs** (`packs.rs`): each block is deflated then sealed (role `pack block`); the index is sealed
  as one record (role `pack index`) and its offset written after it, as now (`MAGIC`, `:28`, becomes
  `HOLONPK2`). `Packs::get` / `block` / `read_index` (`:157`, `:178`, `:371`) open what they read; the
  kept inflated blocks (`KEPT_BYTES`, `:35`) are plaintext in memory as now.
- **Blobs** (`blobs.rs:183-223`): chunks and manifests sealed, chunk names keyed.
- **The sweep** (`sweep.rs:244`) lists loose files by name (`:250-255`) and reads names back as
  hashes. With keyed names it can't, so `there` is keyed **by name throughout**: loose files by
  their file name, packed values by `name(hash)` of each hash the opened index lists
  (`packs.hashes()`, `:257`), so the map never mixes the two. The walk from what each deployment
  names computes `name(hash)` for each hash it reaches and looks it up by that; whatever the walk
  never reaches is unnamed, so removable without knowing its hash. `recent` (puts in the last hour,
  read at `:312`) and the check that no put comes between looking and removing (`:310-311`) are
  keyed by name too, so `put_value` records the name it wrote.
- `migrate::check_copy` (`migrate.rs:250-258`) opens the store with the keyring.

### 5.5 Spools, removed records, backups

- Mail and queue spool files (`mails.rs:615`, read `:678`; `queues.rs:431`, read `:505`): whole files
  under `d:<slug>`, role `spool: mail` / `spool: queue`.
- A removed deployment's records (`deploy.rs:1009-1030`) are an export kept on disk: a whole file
  under `d:<slug>`, role `export: <slug>`.
- **Backups** need more than sealing the file at the end, because the way there copies raw lines:
  - `keep_aside` (`recorder.rs:729-760`) appends the hot file's line bytes to a batch. Those are now
    sealed lines; it **opens** them, and the batch is written as one sealed file (role
    `batch: <slug>`), not a file of lines;
  - `keep_segments_aside` (`recorder.rs:766`) copies segments' lines into the backup: it reads them
    through `read_segment` (which opens and gunzips) and adds them to the batch the same way;
  - `archive_one` (`storage.rs:1155-1168`) reads a batch as text (`:1156`) and hashes its last line
    (`:1159`) for the archive's head; it opens the batch first, so the head is over plaintext, then
    writes the gz export sealed (role `export: <slug>`).
- `holon verify <file>` (`read_export`, `verify.rs:240`) detects
  `HLA1` and opens it with the root key from `--root-key` or the default source and the `keys.json`
  beside the data folder (`--data`), or says which it needs.

## 6. Exports stay plaintext

`GET /deployments/<slug>/export` builds from `Log::lines()` (`verify.rs:133`) and
`Events::lines()` (`:129`), which now return opened lines: the export a person downloads
(`holon export`, `verify.rs:1874`, written by `write_atomically` at `:1977`) is plaintext as today,
and `holon verify` checks it with no key. Who may export is the access model's
(`docs/L4s/access-and-approvals-L4.md`); the export is the one place plaintext leaves on purpose,
and `docs/manual/guides/checking-runs.md` says the person exporting decides how to protect the copy.
`holon export --sealed` (later, §10) could seal it to a recipient's public key.

## 7. Cost

Not measured yet; §9 step 8 measures it as `docs/engineering/themes/measuring.md:55` prescribes.
Expected, from known rates:

- **Per line**: a few KB through XChaCha20-Poly1305 plus base64, about 2–5 µs, under the log's lock.
- **Strict** (1,649 req/s, p50 4.64 ms, a sync ≈ 2.4 ms, `performance.md:506-509`): under 0.1% of a
  run's time; throughput is syncs a second times runs a sync, which this doesn't change.
- **Lazy** (22,388 req/s, ≈ 45 µs a run): 2–10%. The lock is per deployment, so at 22k lines a second
  sealing holds it 5–10% of the time. If measured higher, the line can be sealed with a nonce chosen
  before the lock and only seq/prev patched in — not possible with an AEAD over the whole line, so
  instead the sealed part would exclude seq and prev, sealing `rest` before the lock (`Made`, `:899`)
  and keeping `{"seq":N,"prev":"H",` in clear. That is the fallback, not the plan.
- **State writes**: one seal per value file, under the cell's lock, beside three syncs (`performance.md:520`).
- **Disk**: a line grows by about a third (base64), plus its 40 bytes of nonce and tag (about 54
  once in base64) and the `a1.N.` prefix; a whole file by 49 bytes (magic, scope length, version,
  nonce, tag) plus the scope's name.
- **Reads**: every log read decrypts what it reads. `each_line` (`recorder.rs:1209`) reads every
  segment as well as the hot file (`:1215-1219`), so a sweep pass (`sweep.rs:213`) or a summary
  (`summary.rs:223`) decrypts the whole log it keeps: at a few GB/s, about a second per few GB kept.
  Measured in step 8 on a log at its default keep.

## 8. What it doesn't cover

- **Bound folders and SQLite files.** Their files are the user's, outside the data folder
  (`folders.rs:921-945`, `databases.rs:307`); other programs read them. SQLCipher or a page-level
  VFS is a separate decision.
- **External systems.** Queues, mail servers, databases, feeds: the provider's encryption.
- **Memory and swap.** Keys and opened values are wiped on drop (`Zeroizing`) where they are keys;
  run data in memory is not. A hibernation file or swap can hold it.
- **An attacker running as the server's user.** They read the root key where it is kept (the OS
  store answers that user). `kms:` (§10) narrows this to "while the KMS allows the server".
- **Forgetting and the shared store.** Forgetting `d:<slug>` shreds its log, events, spools,
  exports and waiting runs. Its state values are in the shared store under `server`; the sweep
  removes those nothing names any more within an hour of the deployment's removal (`SPARES_MS`,
  `sweep.rs:40`), but a backup of the store taken before keeps them, readable with the `server`
  key.
- **Losing the root.** Today losing the secrets key loses the secrets; after this, losing the root
  loses everything in the folder. The manual says to back the root up apart from the folder, and
  §2.4 says what that backup costs forgetting.
- **Rolling back a file** (§4): not prevented. Per-deployment store keys would close it at the cost of de-duplication across deployments;
  left open (§10).
- **Metadata.** §2.5.

## 9. Implementation plan

One commit per step, local until the phase ends (memory `push-only-at-important-phases`). Each step's
tests run with `CARGO_TARGET_DIR` set to a short path (memory `long-target-paths-break-msvc`).

**Step 0 — the two files that share a name, and owner-only everywhere** (§1.1, §1.2)
- A test in `logs.rs` tests: a deployment that writes a console line and records a run; its run
  log's file holds only chained lines. Expected to fail today (confirming §1.1).
- `logs::file_of` (`logs.rs:338`) writes `<slug>.console.jsonl` (`.1`, `.2` rolled); `logs-L4.md:76`
  updated.
- `logs::append` (`:354`) and `Exporter::keep` (`heads.rs:335`) open through `append_private`;
  `append_private` (`sealing.rs:306`) and `holon_host::write_atomically` (`lib.rs:564`) call
  `restrict_to_owner` on create on Windows.
- The census (`storage.rs:469`) counts the two files apart.
- Class test `every_file_the_server_writes_is_its_owners_alone`: a server (`tests.rs` harness)
  deploys a flow using state, a collection, a file read, a console line, a waiting run, mail and a
  queue (the spool tests' fakes), runs it, stops; every file under the data and backup folders is
  0o600 / owner-only DACL. Mutation: remove one `restrict_to_owner`.

**Step 1 — the keyring** (`at_rest.rs`, `sealing.rs`, `secrets.rs`, `main.rs`)
- `Cargo.toml`: add `chacha20poly1305 = "0.10"`; `aes-gcm` goes once secrets move (end of step).
- `at_rest.rs`: `Keyring::open(path, &KeySource) -> Result<Keyring, String>` (create on an empty
  folder, refuse a missing or wrong root as §3.2), `Scope`, `Role`, the formats of §4, `Unreadable`,
  `seal_file`/`open_file`/`seal_line`/`open_line`, `Keyring::scope(slug)` creating `d:<slug>` on first use.
- `sealing.rs`: `KeySource` keeps its kinds; `parse` takes the keyring's `key_id` (`sealing.rs:140`);
  account `root-key-<id>`; `Sealer` and `context` go (callers move to `at_rest`); `write_private`,
  `write_aside`, `append_private`, `restrict_to_owner` stay as the plain layer under `at_rest`.
- `secrets.rs`: `SecretStore::open` (`:302`) takes the `secrets` key from the keyring; the plain-file
  and format-2 paths go (`:327-354`); `FORMAT` (`:115`) 4; fingerprints from `prints` (§3.2);
  `Resolved.sealer`/`prints` (`:68-71`) become an `Arc<Keyring>`.
- `durable.rs`: `sealed_for_disk`/`open` (`:746`, `:771`) take the keyring and `d:<slug>`; `host.key`
  (`:96-116`) becomes a `server` record in `keys.json`'s sibling `host.key` sealed as a file.
- `main.rs`: `--root-key` / `--new-root-key` (`:809`, `:838`, usage `:545-573`). The keyring opens
  right after `create_private_dir` (`:957`) and before the store (`FileStore::new`, `:997`, which
  reads pack indexes as it opens), `SecretStore::open` (`:1030`) and `load_deployments` (`:1078`);
  `refuse_to_start` (`:1021-1025`) moves above it, since today it is defined after the store
  opens. `server_files` (`:177-190`) adds `keys.json`.
- `migrate.rs`: `--root-key` (`:16-26`); key files moved as today (`:291-310`).
- `selfmanage.rs:244-250`: purge forgets the root from the source in use.
- Tests (rewrite of `secrets.rs:1300-1460`, `sealing.rs:427-457`, `migrate.rs:341-422`):
  `a_keyring_opens_only_under_its_root` (wrong root, missing root with data, another folder's
  `keys.json`: each refuses, nothing is written), `a_wrapped_key_opens_only_for_its_scope_and_version`,
  `a_value_sealed_for_one_role_does_not_open_as_another`.
- Live: `holon serve` on a scratch folder with `--root-key file:…`, set a dummy secret, restart, a
  run reads it; restart with another key file: refuses (memory `never-print-secret-properties`).

**Step 2 — tables** (§5.3)
- Every writer and reader in §5.3's list through `at_rest`. Tests: each table round-trips sealed,
  and a plain table where a sealed one belongs is refused (`Unreadable::NotSealed`).

**Step 3 — the run log, events, console lines, line logs** (§5.1, §5.2)
- As §5.1; `LineAt.len` on disk. The recorder tests that edit raw files (`recorder.rs:2222-3227`, the
  edits at `:2282`, `:2313`, `:2523`, `:2565`, `:2619`, `:2814-2818`, `:2973`) go through a test helper
  `at_rest::testing::{open_lines, write_lines}` that opens a file to plaintext lines and seals them
  back, so each keeps testing what it tests (a cut line, a changed line, a broken chain).
- `tests.rs:1178`, `:1265`, `:1277` (reading `kept.jsonl`), `events.rs:287-288`, `summary.rs:301-316`,
  `logs.rs:456-490`: the same helper.
- Mutation: `each_line` without opening — the sweep and summary tests fail.

**Step 4 — the store** (§5.4)
- The `AtRest` trait, given to `FileStore::new`; packs `HOLONPK2`; blobs; keyed names in 64 hex
  (the census and `is_id` unchanged); the sweep by name, packed values included. Tests in `holon-host` (`packs.rs:539-550`, `lib.rs:2053`, `:2149`) with a test `AtRest`;
  the sweep tests in `sweep.rs` keep their counts.
- `wasm`: unaffected: the bridge depends on the pure crates only, never `holon-host`
  (`crates/holon-wasm/Cargo.toml:12`).

**Step 5 — spools, removed records, backups; `holon verify` with a key** (§5.5)
- `holon verify <file> [--root-key …] [--data …]`; `read_export` (`verify.rs:240`) opens `HLA1`.
- Tests: an archive written by `archive_one` verifies with the key and says what it needs without
  it; `tests.rs:7829`, `:8030`, `:8115`, `:8211` (export files) unchanged (exports are plaintext).

**Step 6 — the class test**
`no_file_the_server_writes_holds_run_data_in_clear`, in `tests.rs`:
- one flow per writer of §1's table (a route with state, a collection, a file read from a bound
  folder, a console line, a waiting run, mail and a queue through the spool fakes, an action; a
  removed deployment; a backup archive via `[storage]` policy), each fed a canary `canary-<random>`
  in its request and writing it into whatever it keeps;
- run, cut the log back past `HOT` (so segments exist), sweep and pack, stop cleanly;
- walk the data and backup folders: in every file, and in every gzip or deflate stream found in one
  (the segment and archive gzip, pack blocks), the canary appears in no form: raw, JSON-escaped,
  base64, hex;
- the allow-list of §2.5 is checked to hold no canary too (it holds no run data by design);
- the export of the same deployment does hold it (the control: the canary did reach the record).
- Mutations: unseal one writer each (log line, segment, table, store value, spool, backup batch);
  each fails.
- With it, the grep test `no_writer_under_the_data_folder_bypasses_at_rest` (in
  `crates/holon/tests/`; here, not earlier, because the log, spools, records and backups are only
  converted in steps 3–5): the server's sources call `sealing::write_private`, `write_aside`,
  `append_private`, `holon_host::write_atomically`, `OpenOptions` and `fs::write` only from
  `at_rest.rs` and an allow-list, each with its reason: `storage.rs`'s policy writer, `heads.rs`,
  `claims.rs`'s head writer, `stopping.rs` (§2.5), and the files written outside the data folder on
  a person's behalf: the CLI's export (`verify.rs:1977`), `migrate.rs`, `selfmanage.rs`. Mutation:
  a direct `write_private` in `persist.rs`.

**Step 7 — rotation and forgetting** (§2.4)
- `holon key status` (`GET /admin/keys`): the root's source and id, each scope's current version and
  versions in use, never key bytes (memory `never-print-secret-properties`).
- `holon key rotate --root --to <source>` (offline, server stopped: reads the old root, writes the
  new one to `<source>`, re-wraps `keys.json` with `write_private`, then forgets the old root only
  when asked, `--forget-old`).
- `holon key rotate <scope>` (`POST /admin/keys/rotate`, admin only, `can.rs`): a new version, used
  from the next write; `--rewrite` re-seals the scope's files in the background (the store and log
  files are rewritten whole anyway at cut-back and pack).
- `holon remove <slug> --forget` (and the editor's remove dialog, a checkbox): removes
  `d:<slug>` from `keys.json`, records it under `forgotten`, and rotates the root (§2.4), saying
  that a kept copy of the old root still opens earlier copies of the folder. Removing a deployment
  without it keeps its key, so its removed records stay verifiable (`deploy.rs:1002-1030`).
- `names` is not rotatable (§2.6); `holon key rotate names` says why.
- Tests: `a_rotated_root_opens_everything_and_the_old_root_nothing`,
  `a_new_data_key_version_seals_new_writes_and_old_records_still_open`,
  `a_forgotten_deployment_is_unreadable_everywhere_and_says_so` (its log, segments, archive, removed
  records: `Unreadable::Forgotten`), and the chain across a rotation: `holon verify` on an export
  spanning versions matches.
- Access: rotation and forgetting are admin acts → coordinate with "Holon Typescipt and Principles"
  before touching `can.rs` (plan rule).

**Step 8 — measure, docs, e2e**
- `holon load stress-test`, before/after in alternating rounds (`measuring.md:55`), strict, lazy,
  unrecorded; results in `performance.md` with the build ids. If lazy loses more than 10%, §7's
  fallback.
- Docs: `docs/manual/reference/server.md` (§ "The secrets key", `:357-369`, becomes "Keys": the root,
  `keys.json`, rotation, forgetting, what an attacker with the folder learns), `reference/cli.md`
  (`:175-176`, `--root-key`, `holon key`), `guides/checking-runs.md` (verify of an archive),
  `docs/engineering/capabilities/secrets.md` (`:37`, `:48-53`, `:70`), `docs/L4s/holon-home-L4.md`
  §3 (`:71-113`), `README.md`'s record bullet ("encrypted at rest under keys you hold").
- `npm run e2e` locally (memory `worktree-node-modules-lacks-packages`): every sample deploys and
  answers on an encrypted folder (`editor/e2e/sweep.e2e.ts`), and the host e2e
  (`editor/e2e/host.e2e.ts`) restarts the server on its folder.
- **No new sample flow.** CLAUDE.md asks one per feature; this one changes no flow and no node.
  Every sample is its test (the e2e sweep runs them all on a sealed folder), and the commit says so.
- Live: a scratch server (`target/release/holon.exe`, a port no other session holds, memory
  `sample-ports-held-by-other-sessions`) runs the stripe sample and a stateful one, then
  `rg -a <canary>` over the folder finds nothing, `holon export` + `holon verify` passes, a root
  rotation and a restart keep everything readable.

**Order and size.** Step 0: a day. Steps 1–3: about a week (the recorder is most of it). Step 4:
three days. Steps 5–6: two days. Step 7: three days. Step 8: two days. About three weeks, of which
steps 0–3 and 6 alone (log, tables, waiting runs, the class test) are the part a pilot's
questionnaire needs; the store, rotation and forgetting can follow in that order.

## 10. Open

- **`kms:` root.** `KeySource::Kms { uri }`: the keyring's data keys wrapped by the KMS
  (`Encrypt`/`Decrypt` of 32 bytes; AWS KMS first), so the root never reaches the server. Tested
  against a KMS emulator in WSL Docker (memory `no-aws-account`). Fits after step 7, when a pilot
  asks.
- **Rollback protection** (§4): a generation counter per table, sent to the witness with the heads
  or kept outside the folder, so an older sealed table is refused.
- **Per-deployment KMS keys**, destroyed on forget: shredding that reaches every copy (§2.4).
- **Per-deployment store keys** (§8): the price is the cross-deployment de-duplication; worth it if
  forgetting must reach state values in old backups.
- **Sealed exports** to a recipient's public key (`holon export --to <age-recipient>`).
- **Hiding sizes and counts** (padding, fixed-size segments): not planned; §2.5 says what shows.
- **Whether `holon.toml` names the root's source** (`[keys] root = "env:HOLON_ROOT_KEY"`), so `serve`,
  `migrate`, `verify` and `uninstall` agree without each being passed the flag.
