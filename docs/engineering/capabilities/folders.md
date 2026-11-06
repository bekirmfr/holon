# Folders

Files on the host a flow reads and writes by name: an admin binds the name to a real folder, the
flow never sees a path, and every run stays checkable by `holon verify` however big the files
are. The design is the [named folders L4](../../L4s/named-folders-L4.md) (NF, with each slice's
"built" notes); commit locks, claims and run locks on a folder are the
[databases L4](../../L4s/databases-L4.md) §7; the sweep's cost is R5 of the
[resilience audit](../../L4s/resilience-audit-L4.md). The user's side is the manual's
[files guide](../../manual/guides/files.md).

## 1. What it is

```ts
import { flow, route } from "holon"
import archive from "folder:archive"
import logs from "folder:logs"

flow("log-archive", { port: 8124 })

route.get("/logs/:name/tail", (req) => logs.lines(req.params.name, { from: -20, limit: 20 }))

route.post("/archive", (req) => {
  const log = logs.file(req.json.name)
  archive[`${log.name}.${log.hash}`] = log
  return { archived: `${log.name}.${log.hash}`, size: log.size }
})
```

`import logs from "folder:logs"` names a folder; a folder reads and writes as a collection does,
by file name ([LANGUAGE.md](../../LANGUAGE.md) §5.4): `inbox[n]` is `file_read` (the text as the
run began), `reports[n] = text` `file_write`, `delete reports[n]` `file_delete`, `.list`
`file_list`, `.file` `file_handle` (`{folder, name, size, hash, modified}`, no bytes), `.lines`
and `.slice` a range, `reports[n] = h` or `.copy` `file_copy`, `.create` and `.create_copy` the
same only if no file has that name. Reads ask `FileRead("logs")`, writes `FileWrite("archive")`;
`on.file.created(inbox, …)` (a run per file, `FileArrival`, `arrivals.rs`) is in
[triggers and clocks](triggers-and-clocks.md).

- `crates/holon/src/folders.rs`: bindings (`Folders`, `Folder`, `Bound`, a deployment's view
  `Scoped`), confinement, reads (`Readable`), staged writes (`stage`, `Staged`, `Ready`), file
  limits, the sweep, `holon folder` (`cli`).
- `blobs.rs`: the texts runs read, kept for their logs. `ranges.rs`: a range's proof.
- `locks.rs`: commit and run locks; `claims.rs`: claims on files and folders.

## 2. How it works

- **Bound by an admin.** `holon folder add NAME PATH [--read-only] [--keep hash|30d] [--cap SIZE]
  [--shared] [--lock run] [--for SLUG]` (or the admin page, `/admin/folders`) writes
  `folders.json` (`--folders`, owner-only). A `Folder` is its real path, `access` (`Read` or
  `ReadWrite`), `keep`, `cap`, `shared` and `lock`; a binding is the server's or one
  deployment's, which sees its own first. The server's own data, or a folder holding it, can't be
  bound. A deploy is refused until each folder it needs is bound, read-write where it writes.
- **Confined.** `bad_file_name` refuses before the OS sees it: empty, `\`, `:`, a root, `.` or
  `..` parts, a part ending in a dot or space, Windows' reserved names, the host's `.holon-tmp-`
  files. A name is walked to a part at a time: on Linux with no link followed, even one to inside
  the folder (`walk_beneath`), each missing folder made only in the one just checked, and the file
  acted on through its folder as opened (`Placed`), so a link swapped in afterwards leads nowhere;
  elsewhere each folder is checked to be inside before one is made in it, and the file must be
  inside once links are followed.
- **Reads** are answered on the spot from the folder as the run found it, as text (UTF-8, up to
  `inline_file_bytes`; anything else fails the run, saying to read a handle), and kept with the
  run's reads with the file's hash and size (`Seen`). A text up to `LOG_INLINE_BYTES` (4 KiB) is
  in the entry; past it, in the blob store, the entry naming it (`"blob"`). With `keep: hash`
  only the hash is kept; `keep: bytes for 30d` keeps the text for its window.
- **Ranges** read only their bytes, up to `inline_file_bytes`; the entry keeps with them the
  BLAKE3 chaining values outside the range, which recompute the file's hash (`ranges.rs`), so the
  run checks from the range alone.
- **Writes** are effects. `stage` writes each file's bytes beside its name and syncs them
  (`fill_beside`) before any lock; at commit, under the commit locks, what is left is the cap
  (`fits`) and a rename, or for a create-only write a hard link, which fails if the name is taken.
  A copy by handle streams the source host to host and lands only if it still has the handle's
  hash; its handle's folder must be one the deployment reads. The log keeps a write by hash and
  size, never its text.
- **Limits** (`FileLimits`, the host's): `inline_file_bytes` 1 MiB (ceiling 16 MiB),
  `max_file_read_bytes` 64 MiB (1 GiB), `max_file_write_bytes` 1 GiB (1 TiB), `max_files_per_run`
  1,000 (100,000). A read past one fails the run as it reads; writes past one fail it before
  anything commits (`past_write_limits`). A write that didn't land (cap, changed source, name
  taken) answers 500 (`WRITE_FAILED`).
- **Shared with other runs.** A file read and written is checked at commit (`still_as_read`
  reads it again); per-file commit locks, shared for reads, exclusive for writes, with the folder
  held by intent (`locks.rs`), and for a `--shared` folder an OS lock on
  `.holon-tmp-commit.lock`; a run about to call out claims the files it read and may write
  (`ClaimKey::File`, a listing `ClaimKey::Folder`); `--lock run` holds the folder for whole runs.
- **Kept texts** (`blobs.rs`, `<store>/blobs/`): filed under the BLAKE3 of their bytes, cut into
  content-defined chunks (`CHUNK_MIN` 16 KiB, `CHUNK_AVG` 64 KiB, `CHUNK_MAX` 256 KiB), each chunk
  by its hash, every read checked. `folders::sweep` removes the texts no kept run refers to within
  its folder's window, sparing `SWEEP_SPARES_MS` (5 minutes), with the server's other sweeps
  (`[storage] sweep_every`, 10 minutes) or `holon folder sweep`.
- **Checked offline.** `holon export` ships the kept texts (`--max-files-bytes`,
  `--without-files`); `holon verify` answers reads from them, then `--files DIR` by hash, else
  skips the run, saying which file it read.

## 3. Why it is this way

- **Only an admin binds a path**: a deploy that names one would let any editor read the secrets
  key; a moved flow changes the binding, never its text (NF §2).
- **A folder is a collection whose items are on disk** (NF §1): the same reads at the start and
  writes at the end, so nothing new to learn and the same commit check.
- **Hashes, never big bytes, in the log** (NF §5): a write costs one line whatever its size;
  verify runs it again. A deleted text loses re-running, never tamper evidence.
- **Chunks of 64 KiB, not ~1 MiB from 4 MiB** as designed: a run reads 1 MiB at most, so no blob
  is bigger (NF §11, slice 3).
- **`max_file_write_bytes` 1 GiB, not the designed 64 MiB**: a copy by handle is how a big file
  moves, so the default doesn't stop one (NF §11, slice 5).
- **The slow part before the lock.** Filling a file happens before the commit locks; under them,
  a check and a rename ([databases L4](../../L4s/databases-L4.md) §7.1).
- **The sweep holds up no run** (R5, `42a75f9`): it read every deployment's whole log under its
  lock every ten minutes (a 60 MB log held its runs up) and parsed every entry, about 200 MB, even
  with no folder: the soak's 45 MB memory steps. Now it is skipped when the blob store holds
  nothing old enough (`Blobs::any_put_before`), reads each log a line at a time with the lock
  released (`LogCell::lines_holding`, `each_line`), and parses only lines holding `BLOB_KEY`
  ([Resilience](../themes/resilience.md#g6-no-run-waits-for-a-sweep)).

## 4. Reach and limits

- **Text only, read whole.** No bytes value: a binary file is copied or sent by handle, never
  read ([host capabilities L4](../../L4s/host-capabilities-L4.md) §1).
- **Not atomic across files.** Each file lands by its own rename; one that fails leaves the
  others landed, and the run says which didn't. The directory isn't synced after the rename.
- **A cap walks the folder** (`usage`) at each write that lands in a capped folder: its cost
  grows with the folder.
- **A listing looks at 100,000 files at most** (`MAX_WALK`).
- **Programs outside Holon** don't claim: a file changed between a run's read and its commit
  sends the run round again; one changed after the commit check, before the rename, isn't seen.
- **Local and shared paths only**: no remote backends (SFTP, buckets), designed in
  [host capabilities](../../L4s/host-capabilities-L4.md) §6, not built.
- **Rebinding asks no one.** The grant names `FileRead("inbox")`, not a path, so binding the name
  to another folder changes what the next runs reach without a new approval; removing it stops
  the deployments that use it until it is bound again. Rebinding as a grant change is designed
  ([host resources L4](../../L4s/host-resources-L4.md) §3), not built.

## 5. What the tests hold

- folders.rs: `a_name_stays_inside_its_folder`, `a_link_out_of_the_folder_is_never_followed`,
  `a_write_through_a_link_makes_nothing_outside_and_links_arent_followed`,
  `a_folder_swapped_for_a_link_after_the_check_leads_nowhere`,
  `a_bound_folder_reads_writes_lists_and_deletes_and_nothing_leaves_it`,
  `a_deployment_sees_its_own_binding_of_a_name_and_no_one_else_does`,
  `commits_lock_the_files_they_touch_and_a_listing_its_folder`, `keep_is_bytes_hash_or_bytes_for_a_while`,
  `a_capped_folder_takes_a_write_or_a_copy_only_while_it_fits`, `a_create_only_write_or_copy_never_replaces_a_file`.
- blobs.rs: `a_text_is_cut_where_its_content_says_so_a_change_adds_only_the_chunks_around_it`,
  `a_text_is_kept_once_and_a_grown_one_adds_only_its_new_chunks` (and `any_put_before` before and
  after a sweep); ranges.rs: `a_range_proves_itself_part_of_the_file_and_nothing_else_does`.
- tests.rs: `a_flow_reads_and_writes_bound_folders_and_its_runs_verify`,
  `a_big_file_is_read_by_range_and_copied_by_handle_and_its_runs_verify`,
  `a_journal_keeps_what_its_runs_read_in_chunks_for_a_while_and_stays_under_its_cap`,
  `a_run_past_its_file_limits_fails_and_says_which`, `two_deployments_appending_to_one_shared_file_lose_no_line`,
  `a_folder_held_for_whole_runs_runs_its_writers_one_at_a_time`,
  `no_folder_is_or_holds_what_the_server_keeps_beside_its_files`,
  `a_secret_stays_out_of_files_and_of_what_the_log_keeps_of_them`; recorder.rs:
  `a_log_read_a_line_at_a_time_holds_no_lock_and_sees_one_file_whole` (R5).
- **Samples**: `inbox-report` (reads and writes), `log-archive` (handles, copy between folders,
  ranges), `journal` (kept for a while, a cap), `shared-ledger` (commit locks across
  deployments), `stock-sheet` (a restore leaves files), `drop-box`, `file-audit`, `drop-board`
  (`on.file.*` handlers). Each deploys in `samples.e2e.ts`, and each has a test of its own there:
  `inbox-report`, `log-archive` and `journal` bind temporary folders and verify their runs.
- **Gaps**: no test crashes between a file's rename and the run's record; the sweep's pace under
  a large blob store isn't measured.

## 6. History

Named folders arrived in rc.22 (slices 1–5, with `inbox-report`, `log-archive`, `journal`,
`drop-box`, `file-audit`), commit locks and shared files in rc.27, and secrets kept out of files
in rc.28 (CHANGELOG), before this repository's history starts at `6bc5c68`. Since:

| Commit | Change |
|---|---|
| `7d3cab7` | Binding or unbinding a deployment's own folder is an event on its timeline |
| `5fc292a` | A restore's dry run names the files written since, and an edit outside a run |
| `21e63e6` | A run's performer refuses a request carrying a secret elsewhere (`SecretSends`, in folders.rs) |
| `42a75f9` | R5: the file texts' sweep holds up no run |
| `5b921ed` | How often the server sweeps is a setting (`[storage] sweep_every`) |
| `1e7bcf1` | The host says what it holds, the folders' kept texts among it |
