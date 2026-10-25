# Named folders: files a flow reads and writes, on the host (L4)

**Status:** design, before any code. Written 2026-10-25.

A flow reads and writes files in **named folders**. An admin binds each name to a real folder on
the host; a flow asks for a name, never a path; a deployment is granted it like any capability.
Every run stays checkable by `holon verify`, however big the files: the log records what was
read and written **by hash**, and the bytes, when kept at all, go to a content-addressed blob
store beside it.

```
flow inbox-report
folder inbox
folder reports

route POST /report (req) {
  csv    = inbox[req.json.name]
  rows   = csv_parse(csv)
  report = render_report(rows)
  reports["{req.json.name}.md"] = report
  respond { rows: length(rows) }
}
```

---

## 1. What is already there

- `Capability::FileRead` (a driver) and `Capability::FileWrite` (a sink) exist in
  `holon-core/src/kind.rs`, but **unaddressed**: a leaf declaring `FileRead` doesn't say which
  file, so the server never sources it (`firing.rs`, the note on `extra_sourced`) and never
  performs `FileWrite`. That is the real design question this note answers: *how a flow names a
  file*.
- Addressed capabilities are the established answer elsewhere: `SecretRead(name)`,
  `HttpRequest(host)`, `CollectionRead(name)` / `CollectionWrite(name)`. Consent names *which*
  secret, host or collection, and the grant (`permissions.rs`, `Grant.items`) records it.
- Collections are the closest model. `collection notes` in the header; `notes[k]` reads,
  `notes[k] = v` writes; reads as the run began, writes committed when it finishes
  (`LANGUAGE.md` §6.6); each read logged (`LogEntry.reads`, `LoggedRead {path, request,
  answer}`) so replay answers from the log; a write that raced a changed read runs again
  (`retries`, `key-claims-L4.md`).
- `holon verify` re-runs each entry and already has the vocabulary for what it can't re-run:
  `Rerun::Skipped(why)`, counted, never failed (secrets, resume tokens).
- The store, the log chain and the witness already hash with BLAKE3.
- `holon export` already ships content by hash (`Export.units`: every unit the runs ran).

So: a folder is **a collection whose keys are relative paths and whose items live on the host's
disk**. Most of the machinery is reused; the new parts are the binding, the confinement, and
keeping big bytes out of the log.

## 2. Folders are bound by an admin, asked for by a flow

**On the server (admin only).** A folder is `name → { path, access, keep, limits }`, kept beside
the secrets (`--folders`, owner-only, written from the admin panel and `PUT /admin/folders/<name>`):

```json
{ "inbox":   { "path": "D:\\drop\\in",  "access": "read",       "keep": "bytes" },
  "reports": { "path": "D:\\reports",   "access": "read-write", "keep": "hash"  } }
```

- **Only an admin binds a path.** An editor deploys flows; letting a deploy name a host path
  would let any editor read `secrets.key`. A flow names `inbox`, and the host decides what
  `inbox` is. Moving a flow to another machine changes the binding, never the flow.
- **`access`** is the most a folder allows (`read`, `read-write`); a flow that only reads is
  granted only `FileRead`.
- **Never bindable:** the server's own data folder (store, deployments, accounts, secrets, keys,
  run files) or anything inside it, checked when bound and again at open.

**In the flow.** `folder inbox` in the header names it, exactly as `collection notes` does: a
name, not a permission. `check` derives the capabilities from the leaves:
`FileRead("inbox")`, `FileWrite("reports")`. The two unaddressed variants become addressed
(`FileRead(String)`, `FileWrite(String)`), the same change `SecretAccess` → `SecretRead(name)`
made.

**At deploy.** The manifest lists them; the deployer acknowledges them alongside egress
(`deploy`'s `acknowledge`), and the grant records them. As for every capability, an agent
(`holon mcp`) can't acknowledge one: a person grants a folder, from the editor. A folder the server doesn't have, or
binds read-only when the flow writes, is a deploy error that names it. `optional folder inbox`
works as `optional secret.x` does: run without it, its reads absent.

## 3. The language

A folder reads like a collection, so nothing new to learn:

| Written | Lowers to | Reads as |
|---|---|---|
| `inbox[name]` | `file_read{folder: inbox}(name)` | the file's text, as the run began (absent if none) |
| `inbox.list(prefix: p, after: a, limit: n)` | `file_list{folder: inbox, limit: n}(p, a)` | a page of files in name order: a record of name to `{size, hash, modified}`, as a collection's listing is of key to item |
| `inbox.file(name)` | `file_handle{folder: inbox}(name)` | a handle (§4): the file without its bytes |
| `inbox.lines(name, from: i, limit: n)` | `file_lines{folder: inbox, limit: n}(name, i)` | lines `i..i+n` |
| `inbox.slice(name, offset: o, length: n)` | `file_slice{folder: inbox}(name, o, n)` | those bytes, as text |
| `reports[name] = text` | `file_write{folder: reports}(name, text)` | write it when the run finishes |
| `reports[name] = handle` | `file_copy{folder: reports}(name, handle)` | copy it when the run finishes, host to host |
| `delete reports[name]` | `file_delete{folder: reports}(name)` | delete it when the run finishes |

The same rules as collections: reads at the start, writes at the end (§6.6), one write per name
per flow, the statement forms exposed and performed (§5.10).

## 4. Big files never enter the flow whole

The core's values are text, numbers, lists and records; a 4 GB file can't be one, and shouldn't
be.

- **Small files** (up to `inline_file_bytes`, default 1 MiB) are read as text, UTF-8, bytes
  untouched (no line-ending changes). A file that isn't UTF-8 is an error that says to use
  `.file` (a handle).
- **Handles.** `inbox.file(name)` is the record `{folder, name, size, hash, modified}`. It
  carries no bytes, so a flow can list, filter, route and pass on files of any size. The host
  streams the bytes only where a handle is *used*: `reports[n] = handle` (copy), and, in a
  later slice, an `http.put` body or a mail attachment. Nothing large passes through the core,
  so nothing large is evaluated, memoized or logged.
- **Ranges.** `lines` and `slice` read part of a file: the last 100 lines of a 10 GB log, a
  header, a page. Only those bytes enter the flow, and only those are recorded.

## 5. What the log holds: hashes, never big bytes

The log grows by tens of bytes per file, whatever the file's size.

**Writes need no bytes at all.** A run's write is computed by the run, and `holon verify`
re-runs it. The log records `{folder, name, size, hash}` per write (in `performed`, as an
effect is today), and verify compares the re-run's hash. A 2 GB report costs one line.

**Reads go to the blob store, by reference.** A read is logged as a `LoggedRead` whose answer is
the bytes inline when small (`log_inline_bytes`, default 4 KiB), and otherwise a reference
`{blob: <blake3>, size}`. The bytes go into a **content-addressed blob store** beside the unit
store (`<store>/blobs/`), keyed by their hash:

- **Deduplicated:** the same file read by 500 runs is stored once.
- **Chunked:** files over 4 MiB are cut into content-defined chunks (FastCDC, ~1 MiB average),
  stored by hash, with the blob a list of chunk hashes. A log file that grows by a megabyte a
  day stores the new chunk, not a new copy.
- **Ranges verify against the whole:** a `slice` or `lines` read records the range and its bytes,
  plus the whole file's hash; BLAKE3's tree (bao) can prove a range belongs to that hash, so a
  run that read 100 lines of a 10 GB file is checkable from those 100 lines.
- **Integrity doesn't depend on keeping bytes.** The chain covers the hash. Deleting a blob
  loses replay, never tamper-evidence.

Handles are logged as what they are: `{name, size, hash, modified}`. A copy is logged as a write
whose hash must equal the handle's.

## 6. Each folder chooses what it keeps

Set when the folder is bound (`keep`):

| `keep` | Stores | `holon verify` can |
|---|---|---|
| `bytes` | every read's bytes | re-run any run, anytime |
| `hash` | the hash only | prove *what* was read; re-run when the file is supplied (`--files`) or still in the folder unchanged |
| `bytes for 30d` | bytes, then hash only | re-run within the window; prove after it |

A blob is removed when no kept log entry references it within its folder's window (a sweep, as
the log trims today). Verify reports a run whose bytes are gone as **skipped: "it read
inbox/a.csv, not kept (hash 7c1e… recorded)"**, counted like a run that used a secret.

## 7. holon verify and holon export

- **Export** ships the blobs the runs reference (`Export.blobs`, by hash, as `units` are), up to
  `--max-files-bytes`; `--without-files` ships hashes only. `holon export` says what it left out.
- **Verify** answers each read from, in order: the export's blobs, then `--files DIR` (a folder
  of the originals, matched by hash, not by name), then skipped. Every byte supplied is hashed
  and must match the log; a range is checked by its bao proof. Writes are compared by hash.

## 8. Consistency with the world outside

Other programs write these folders too, without claims.

- **A run reads a snapshot.** Each file is hashed as it is read; the run sees that content.
- **Writes commit atomically** when the run finishes: written to a temporary file in the same
  folder, flushed, renamed over the name. A reader never sees half a file.
- **Read-then-write of the same name is checked.** If the run read `reports[x]` and writes it,
  the commit compares the file's hash with the one read; if another program changed it, the run
  runs again (`retries`), as a collection write does.
- **A write of a name the run never read** replaces whatever is there. A flow that must not
  replace says so (`reports.create(x, text)`), and the commit fails, naming the file, if it
  exists by then.

## 9. Limits and confinement

**Limits** (new `LIMIT_NAMES`, per deployment, host defaults, asked for in the manifest):
`max_file_read_bytes` per run (default 64 MiB), `max_file_write_bytes` per run (default 64 MiB),
`max_files_per_run` (default 1000), `inline_file_bytes` (1 MiB). A folder may cap its own size.
A run past a limit fails, saying which.

**Confinement:** every name is checked before it reaches the operating system.

- Relative only: no leading `/` or `\`, no drive letter, no `..` segment, no empty segment.
- Windows: no `:` (alternate data streams), no reserved names (`CON`, `NUL`, `COM1`…), no
  trailing dot or space; names compare case-insensitively when listing and checking writes.
- No way out through links: the opened file's real path (after symlinks and junctions) must
  still be inside the folder's real path. Checked when the handle is opened, not before, so a
  link swapped in between can't escape.
- Hidden from flows: the temporary files the host writes, and anything that fails these rules
  (`list` skips them).

Every write and delete goes to the activity log as `file`, with the folder, name, size and run.

## 10. The sample

`inbox-report` (`crates/holon-author/samples/inbox-report.holon`): `folder inbox`, `folder
reports`. `POST /report {name}` reads a CSV from `inbox`, counts and totals its rows, writes a
Markdown summary to `reports`, and answers the row count; `GET /reports` lists `reports`. Run
live with the folders bound to temporary directories; deployed by e2e the same way, which also
exports the runs and verifies them with the files kept and with `--without-files` (skipped, by
hash).

## 11. Slices

1. **Folders and text.** Admin binding (`--folders`, admin panel), confinement, the addressed
   capabilities, the grant; `inbox[n]`, `.list`, `reports[n] = text`, `delete`; atomic writes and
   the read-then-write check; reads logged inline or by blob (no chunking yet: a blob is one
   file); writes logged by hash; verify and export with blobs and `--files`; `keep: bytes|hash`;
   the sample. Small files only (`inline_file_bytes`).

   **Built** (`crates/holon/src/folders.rs`, the `inbox-report` sample). As designed, with these
   differences: a file is text up to 1 MiB, a run reads 64 MiB at most, and a read's text is in
   the log up to 4 KiB (past that, in the content store by hash); `holon export
   --max-files-bytes N` keeps the blobs to N bytes in all (oldest run first) and
   `--without-files` ships none, the export saying how many it left out (`files_left_out`); a run whose
   read fails answers 500 ("its run log says why") and performs nothing; a node output carrying
   a text the log keeps by hash is left out of the run's timeline, and a write effect in the
   run's outputs is logged as `{name, size, hash}`, as it is performed. A name is bound for the
   whole server or for one deployment (`holon folder add NAME PATH --for SLUG`, `deployment` in
   `POST /admin/folders`): a deployment sees its own binding of a name first, then the server's,
   and no other deployment sees its own.
2. **Handles and ranges.** `.file`, copy by handle, `.lines`, `.slice` with bao-checked ranges.

   **Built** (`crates/holon/src/ranges.rs`, the `log-archive` sample). The proof isn't the bao
   format: it is the BLAKE3 chaining values of each subtree outside the range, in order, with the
   bytes around it to whole 1 KiB chunks (`blake3::hazmat`), which recomputes the file's hash; the
   log keeps them with the range (`file.range`: `start`, `end`, `text`, `before`, `after`,
   `proof`). A range is up to 1 MiB and counts toward the run's 64 MiB; proving it reads the file
   once. `lines` without a `from` starts at the first; a negative one counts from the end. A copy
   is checked at commit (the source still hashes to the handle's hash) and refused when the
   handle's folder isn't one the deployment reads: a handle is a record, and one can be made up.
   `reports[n] = h` lowers to a copy when `h` is a handle read in the same flow, else
   `reports.copy(n, h)`.
3. **Storage.** Chunking and dedup across files, `keep: bytes for <d>` and the sweep, folder caps.

   **Built** (`crates/holon/src/blobs.rs`, the `journal` sample). The blob store is its own,
   `<store>/blobs/`, not the content store's values (a sweep must never remove a value a
   deployment's state holds): a text is filed under the BLAKE3 of its bytes (for a whole file,
   the file's hash), as a small file naming its chunks, each chunk under `chunks/` by its hash;
   every read checks each chunk and the whole. Chunks are content-defined (a gear hash with
   FastCDC's normalized masks) from 16 KiB to 256 KiB, 64 KiB on average, not ~1 MiB from
   4 MiB: a run reads 1 MiB at most, so a blob is never bigger. `keep` is `bytes`, `hash` or
   `bytes for 30d` (`--keep 30d`; s, m, h or d). The sweep runs every 10 minutes (and on
   `holon folder sweep`, `POST /admin/folders/sweep`): a text is live while a run its
   deployment's log keeps refers to it within its folder's window (a folder now kept by hash
   keeps none; one no longer bound keeps them all), and one put in the last 5 minutes is
   spared, as is any chunk a live text names; a trimmed log entry's texts go the same way.
   An export counts what is gone (`files_not_kept`), and verify skips those runs, saying so.
   A cap (`--cap 10M`) is checked as each write or copy lands, against the folder's files then:
   a write past it doesn't land, and a run whose file write didn't land now answers 500 (its
   log says why). Logs from before keep naming their texts by value hash in the content store,
   and are still answered from there.
4. **Triggers.** `on file inbox` (a run per new file), and handles as `http` bodies.

   **Built** (`crates/holon/src/arrivals.rs`, the `drop-box` sample). The trigger is derived from
   the graph, as a request's and the clock's are: `on file inbox` declares the folder and lowers
   `ingress` to a `file_arrival{folder: inbox}` leaf, whose capability `FileArrival("inbox")` is
   the deployment's trigger (no deploy setting). A thread looks at each folder a running
   deployment watches every second, by each file's size and modified time (nothing is read), and
   a file new or changed since the deployment last saw it starts one run once it is the same at
   two looks; that run reads its handle, as `inbox.file(name)` would, from `ingress`. The files
   there when it is first deployed are seen, not run on (a file dropped a moment later runs); what it has seen is kept
   beside its log, so a file that arrived while it was down or paused runs when it is back; a file
   its own runs wrote or copied there doesn't start a run unless it changes again; a deleted file
   is forgotten. At most 100 runs per look per deployment, one after another. A handle as a body
   is a leaf of its own, `http_upload{method, host}(path, headers, file)` (`http.put(host, path,
   file: h)`), not a guess about a record's shape: a record sent as `body:` is still JSON. The
   run's performer opens the file, checks it against the handle's size and hash before a byte is
   sent and again as it is sent (`{status: 0, error: "changed"}` otherwise), and streams it within
   the request size limit; the log keeps the request with the handle. A handle's folder, for a
   body or a copy, must be one the deployment reads or the one whose new file started the run
   (that file is handed to it). Mail attachments wait for mail.

## 12. Forks, surfaced

1. **`folder` declared in the header, or inferred from use?** Declared, like `collection`: it
   reads better, and an undeclared `x[k]` already means a record lookup.
2. **Where bindings live:** a new `--folders` file, or inside the secrets file. A new file: the
   secrets file is encrypted and its audit is about values; folders are configuration.
3. **Text-only content in slice 1.** Binary files work only through handles (slice 2). The core
   has no bytes value; adding one is a separate decision this doesn't need.
4. **Inline threshold in the log** (4 KiB): small enough that a log of a busy flow stays small,
   large enough that config files and small JSON need no blob.
