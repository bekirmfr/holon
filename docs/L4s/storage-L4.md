# Storage: sweeping, watching, and what happens when the disk fills (L4)

**Status:** design, before any code. Written 2026-11-04.

A Holon server keeps things: each deployment's log, the states its runs left, the texts of files
they read, and, in memory, its caches and live states. Sweeps keep this bounded
([resilience](../engineering/themes/resilience.md), G9), but a host's admin today can't see how
much is held, by which flow, or how near the disk is to full, and can't say what should happen
when it fills. This note designs that: a setting for the sweeps, a view of what the host holds
(the whole server, and each flow), a warning with a threshold the admin sets, a choice of what the
server does when the disk is full, and places to send old runs before they are let go.

---

## 1. What is already there

- **Sweeps** (`sweep.rs`, `folders.rs`): the store's, every ten minutes, paced, sparing the last
  hour (`sweep::EVERY`, `SPARES_MS`, `PACE`); the folders' kept file texts, every ten minutes
  (`folders::SWEEP_EVERY`). An admin's, now: `holon store sweep`, `holon folder sweep`, the admin
  page's **storage** section. Neither interval is a setting.
- **Retention**: each log keeps its newest 500 runs by default (`--keep-runs`), or what its flow's
  `keep` line and the deploy settings say, approved past the host's default; its file holds up to
  twice that. A run cut by retention is gone; the witness hears a `trim`.
- **Caches** with sizes that are settings (`--flow-cache`, `--value-cache`), counted in memory.
- **Pausing**: a paused deployment's routes answer 503 and its clocks stop; the host already pauses
  one itself when a grant it needs is revoked, and the timeline says so ("by the host").
- **Exports**: `GET /deployments/<slug>/export` writes a log's lines and every unit its runs ran;
  `holon verify` checks one offline. An export is a backup that proves itself.
- **The policy** (`holon.toml`, `[limits]`, `[ceilings]`, `[grants]`): read at start and on
  `POST /permissions/reload`; shown by the editor, not edited by it.
- **Nothing reports** memory, disk, or what a flow holds. `free_gb` in the soaks came from `df`.

## 2. Settings: `[storage]`

In `holon.toml`, beside the rest of the host's policy, and editable from the admin page (`POST /admin/storage`, which
writes the file and reloads it; a file that doesn't parse changes nothing, as now):

```toml
[storage]
sweep_every = "10m"        # both sweeps; "off": only an admin's
warn_at = "85%"            # of the data folder's disk in use; or "5G" free
full_at = "95%"            # where `when_full` acts; or "1G" free
when_full = "warn"         # "warn" | "pause" | "roll"
backup = "/mnt/archive"    # where runs go before they are let go (§4); none: they are deleted
```

- **`sweep_every`**: how often the server sweeps the store and the folders' texts. `off` leaves
  sweeping to an admin; the host view then says how much a sweep would free, and warns when it
  passes a tenth of the disk.
- **`warn_at`, `full_at`**: a percentage of the disk in use, or space left. Measured on each disk
  the server keeps things on (the data folder, and the store, the logs or the folders where they
  are elsewhere), every `SYNC_EVERY` (one `statvfs`, nothing read). Past `warn_at`: a warning
  (§3). Past `full_at`: `when_full`. Back under `warn_at`: the action ends (a gap between the two,
  so it doesn't flap).

### What happens when it fills: `when_full`

| Value | What the server does | What it keeps | What it costs |
|---|---|---|---|
| `warn` | Nothing but the warning, louder | Every promise | The disk may fill: a strict run then fails its sync and answers 500; a lazy one may lose its record |
| `pause` | Pauses every deployment that records runs, by the host ("the disk is full"); unrecorded routes go on. Resumes them once under `warn_at` | Every record already kept, and every run answered is recorded | Its flows stop answering until space is made |
| `roll` | Cuts the oldest runs, server-wide, from the logs that keep the most, down to a floor (20 runs each), sends them to `backup` first if there is one, and sweeps at once | The newest runs everywhere; the cut ones in the backup | A deployment keeps fewer runs than it was approved for; its timeline and the witness say so |

The note recommends `warn` as the default (nothing changes behaviour without an admin choosing
it), with the admin page asking for a choice the first time the warning shows.

## 3. Seeing what the host holds

One endpoint, `GET /admin/host` (an admin's), read by the admin page's **storage** panel, the
terminal editor's admin view and `holon host`; and `GET /deployments/<slug>/holds`, its share for
one flow, read by the flow's page in both editors and `holon host --for SLUG`.

**The whole server:**

- **Memory**: resident and peak (`/proc/self/status`), each cache's use against its size, and the
  part no flow accounts for (the server, and what the allocator keeps).
- **Disk**, for each disk it keeps things on: size, in use, free, against `warn_at` and `full_at`.
- **What it keeps there**: the content store (values, bytes), the logs, the folders' kept texts.
- **Sweeps**: for each, the last (when, what it removed and freed, how long it took), the next, the
  setting; and what a sweep would free now, as of the last one.
- **Warnings** (§3.1).

**Each flow** (a row per deployment in the panel, ordered by what it holds; its own page shows
its row):

| What | How it is counted | Cost to count |
|---|---|---|
| Its log on disk | the file's size and lines | a `stat` |
| Its log in memory | the entries it keeps, by their footprint | kept as entries come and go |
| Its live state in memory | the cells' footprint | kept as the state changes |
| Its states on disk | the values its live state and its kept runs name, by the last sweep's walk: a value first reached from it is its own; one another flow reached first is counted as shared | none: the sweep walks them already |
| Its flow in the cache | its closure's footprint; units another flow shares, as shared | kept by the cache |
| Its file texts | the kept texts its runs name, by the folders' sweep | none: the sweep reads them already |

What a flow holds on disk is "as of the last sweep", and says when that was. Counting it any
other way means walking the store on each view, millions of files on a busy host.

### 3.1 Warnings

- **Disk** past `warn_at` (and past `full_at`, with what `when_full` did): on the admin page and the
  home page for admins, in `holon host`, printed once in the server's log, and in the activity log.
- **A cache at its size** for most of the last hour (it is evicting what runs read again).
- **Sweeping off** and what a sweep would free past a tenth of the disk.
- **A flow growing fastest**: the deployment whose share grew most since the last sweep, when the
  disk is past `warn_at`, so the admin knows where to look.

Each warning says what to do: raise the disk, lower a flow's `keep`, set a `backup`, choose a
`when_full`.

## 4. Where old runs go: `backup`

Runs leave a log in two ways: retention cuts them as new ones come (always), and `roll` cuts more
when the disk is full. With a `backup`, both write the runs they cut there first, as an export a
`holon verify` checks (the lines, the units they ran, the states they left): a cut run is moved,
not lost.

| Kind | `backup =` | What it reaches | When |
|---|---|---|---|
| A folder | `"/mnt/archive"` | An external or second drive; a folder a cloud client keeps in sync (Dropbox, Google Drive, OneDrive, iCloud) | First |
| Object storage | `"s3://bucket/prefix"` (any S3-compatible endpoint, credentials from the server's secrets) | Cloud storage directly | Second |
| Holon Cloud | `"holon-cloud"` | Kept, indexed and checkable by Holon | Upcoming: shown as coming, not offered |

A backup that can't be written (a drive gone, a bucket refusing) is a warning, and the runs are
kept until it can be: retention waits rather than delete what it was asked to move, up to the
`full_at` point, where `when_full` decides.

## 4.1 Keeping old records smaller: lossless compression

Measured on the soaks' data folder (2026-11-04): a 56.5 MB log (`notes-api`'s) is 1.96 MB gzipped
(29 times smaller); 2,000 of the store's values, 717 KB of bytes (358 B each) but about 8 MB on
disk (each file takes a 4 KB block), are 128 KB packed together and gzipped (about 60 times
smaller than on disk). `flate2` is already in the build.

Lossless, so nothing a record proves changes: a log's chain hashes each line's bytes, and a line
read back from a compressed segment is the same bytes; a value is named by its hash, and read back
the same.

- **Archives** (`backup`, §4): written compressed. The export's format with gzip around it;
  `holon verify` reads either.
- **Logs, in segments**: a log's file becomes its live segment (appended to as now) and sealed
  segments behind it, each compressed when sealed (past a size, say 4 MB of lines). Reading old
  runs (history, exports, verify, the sweeps) decompresses a segment at a time. A cut-back then
  deletes whole sealed segments instead of rewriting the kept lines, so it gets cheaper too (R4's
  machinery shrinks to the live segment). The chain runs across segments unchanged.
- **The store, in packs**: values older than what a sweep spares are moved into pack files (many
  values, compressed together, with an index by hash), the files they were removed; a read looks
  in the loose files, then the packs' indexes. This is R6's second half (7.7 files a write), and
  the sweep then rewrites a pack without what nothing names instead of unlinking files one by one.

Each is a format change to what is on disk: a server reads the old form and the new, and a
downgrade is refused with a reason, not misread.

## 5. Reach and limits

- **The disk measures are the filesystem's**: another program filling the disk counts too. That is
  the point: the server reacts to the disk it has.
- **Per-flow disk shares are approximate**: content addressing shares values between flows and
  between runs; a shared value is counted once, as shared, not split.
- **Memory per flow** covers what Holon allocates for it (log entries, live state, its flow in the
  cache); a run's own allocations are brief and not counted; the rest is the server's.
- **`pause` and `roll` are blunt**: they act on every flow that records runs. Per-flow choices (this
  one may roll, that one must pause) are a later step if a host asks.
- **No backup of the whole data folder** (the accounts, secrets, deployments file): that is a
  machine's backup, not this.

## 6. Build order

1. **`[storage] sweep_every`**, both sweeps, shown on the admin page; `off`. **Done**: one housekeeping loop (`storage.rs`, looking every 10 s), `GET /admin/host`'s `sweeps`, `POST /admin/storage` (it writes `holon.toml` with its comments kept, then reloads), `holon policy --sweep-every`, the admin page and the terminal editor's admin view.
2. **`GET /admin/host`**, `holon host`, the admin page's panel: memory, disks, what is kept, sweeps. **Done** (`storage.rs`): the store and the folders' texts counted by the housekeeping thread at its first look and after each round; logs, caches, memory (`/proc/self/status`) and disks (`statvfs`, one entry a disk, how full for the server: what isn't free to it) read when asked. Found on the way: what a crash leaves aside (`….tmp-<pid>-<n>`, `….cut-<pid>-<n>`) was never removed (60 MB in the soaks' data folder); the count removes it once older than 15 minutes. Measured live: the store's 7,974 values took 31.3 MiB on disk for 2.2 MiB of bytes (whole 4 KiB blocks), what step 10 is for.
3. **Per-flow shares**: counted in the sweeps' walks and as entries, states and closures change;
   `GET /deployments/<slug>/holds`; the panel's rows, each flow's page in both editors.
   **Done**: the store's sweep walks each deployment's names in turn (a value is the first's to
   reach it, shared if another reaches it too) and notes the file texts each log names on the
   same pass over the logs; a log counts the bytes its kept entries were written as. **Found:** a
   log keeps its runs in memory whole, 1.1 to 3.6 times their lines (`measure_log_memory`): the
   soaks' `notes` listing holds about 86 MB, `crunch` about 52 MB, and nothing bounds it: a
   deployment approved to keep 20,000 runs (the ceiling) of a 50 KB line would hold about 3.6 GB.
   See resilience-audit-L4.md, R8: fixed, a log on file keeps its newest 32 runs in memory.
4. **`warn_at`** and the warnings. **Done**: `Threshold` (`85%` full, or `5G` free), set with `POST /admin/storage` and `holon policy --warn-at`; `storage::warnings` (a disk past it, with the flow that grew most between the store's counts; a cache that let go of something to fit at most looks of the last hour, counted by what each cache lets go of; sweeping off with the store past a tenth of its disk), looked at every 10 s and said once as each starts and ends; `GET /admin/warnings` for the terminal editor. The admin page's disk meter is red when its disk is warned of.
5. **`full_at` and `when_full`**: `warn`, `pause` (and resuming), `roll`. **Done**: `storage::Sweeps::act`, every look. `pause` marks what it paused in its grant (`paused_full`, so a restart keeps it) and resumes only that, once every disk is under `warn_at`; a person's resume while full is paused again. `roll` cuts the biggest log on disk to half (never below `ROLL_FLOOR`, 20), one a look, the file cut back by the periodic sync (R4), and sweeps once, at the first cut of a full disk: sweeping after each cut held the next up for the length of a sweep (73 s on the soaks' data: one log cut in 45 s, then 192 → 39 MB of logs in 120 s once changed). Run live on the soaks' data at the defaults (this container's disk 98% full for the server): `roll` cut `crunch-lazy` and `notes-api` first, then kept runs as granted again with room made; `pause` paused the 9 flows recording runs (a strict route answered 503, an unrecorded one 200) and resumed them.
6. **`backup` to a folder**: retention and `roll` export what they cut there first. **Done**: a cut-back (the periodic sync's, outside the lock, or one made holding it) writes the lines it lets go of to `<backup>/<slug>/holon-archive-<first>-<last>.jsonl` first (`recorder::keep_aside`, renamed in, so the same cut made again writes the same file); if it can't, it doesn't cut (the in-lock cut appends the waiting line as any is), unless `roll` is acting past `full_at`. The housekeeping makes each batch an export (`verify::export_of`, shared with `GET …/export`) gzipped, `holon-archive-<slug>-runs-<first>-<last>.json.gz`, and removes the batch. Every file the server writes there starts with `holon-archive-` (`storage::ARCHIVE_PREFIX`), and it only exports or removes a file named so, with a batch's exact shape: the backup may be a drive or a synced folder holding other things. Each log is given the backup by the housekeeping, at start and every look.
7. **`backup` to object storage**; Holon Cloud named as coming.
8. **Compressed archives** (with 6, if small enough to go with it). **Done** with 6: gzip (`flate2`), `holon verify` reading either. Measured live on the soaks' data: `hello-api` 10,669 runs, 51 MB as JSON → 3.9 MB (13×, 361 bytes a run); `notes-api` 10,049 runs, 618 MB → 20 MB (30×); `holon verify` of a batch: chain unbroken, 2,744 runs re-run, each as its entry says.
9. **Logs in compressed segments.** **Done** (recorder.rs): a log keeps its newest `HOT` (128) runs in its file as written, appended and synced as before, so a run's path is unchanged; a cut-back (the sync's, outside the lock, or one made holding it, alike: `split`) seals the older runs it keeps into `<slug>.sealed/<first>-<last>.jsonl.gz`, renamed in before the file is replaced, and lets go of whole segments none of whose runs it keeps (to the backup first). Opening a log reads its segments, then its file, each run once (a cut that crashed between the two leaves runs in both: the file is written again without them), and asks for a cut when its file holds more than a cut leaves, so an idle log seals too. Reads of older runs (pages, exports, `holon verify`, the sweeps' pass, `shows`) read the segments they need, the file opened under the lock first so a cut meanwhile hides nothing. A log keeping no more than 128 runs seals nothing. Measured on a copy of the soaks' data folder: the logs went from 192 MB to 37 MB within 8 seconds of starting (`notes-api`'s 372 older runs, 18.6 MB as lines, in a 624 KiB segment; its newest 128, 6.1 MiB, as written); an export from it verified; throughput, the build before against this one, two rounds: lazy `hello` 17,037–18,078 / 18,015–18,242, the `notes` listing 1,758–2,018 / 2,164–2,183, strict `hello` 6,434–6,496 / 6,824–7,302; a page of 200 runs (past the 128 as written) 492–931 / 528–792 ms.
10. **The store's old values in compressed packs.** **Done** (holon-host `packs.rs`): the server's own sweep moves the values it keeps that are older than it spares into a pack (deflated blocks of about 64 KiB, an index of 52 bytes a value at its end, written whole and renamed in, then the values' files removed, paced); a read looks for a value's own file, then the packs' index, in memory; a packed value nothing names any more is swept by rewriting its pack (written with no lock held, the index then moved to it at once under the store's lock, where a value put again since the look stays: so no read misses a value kept and no put comes between; a pack is likewise written before its index is taken, so reads go on while it syncs). Measured on a copy of the soaks' data folder: 140,026 files, 562 MB on disk, became one pack of 1.0 MiB (the 7,974 values kept, 2.2 MiB of bytes; the rest swept). The `notes` listing with no value cache (every read from disk) answered 636–672 requests a second from files of their own, 74–82 from the pack keeping 16 inflated blocks (a read inflated a block again at most reads), 811–825 keeping 8 MiB of them; with the default value cache, 2,201–2,355, as before.

Each step lands measured (what the counting costs a request: nothing on the request path but
counters), with its tests, its docs, and a sample where a flow shows it (a flow that fills its
log fast, for steps 3–6).

## 7. Questions

- **Q1. The admin page edits `[storage]`.** Today the editor shows the policy but doesn't edit it.
  Recommended: it edits this section only, writing the file and reloading.
- **Q2. The default `when_full`.** Recommended: `warn` (no behaviour changes until an admin chooses),
  the first warning asking for a choice.
- **Q3. `pause` resumes by itself** once under `warn_at`. Recommended: yes, and the timeline says
  both.
- **Q4. What `roll` cuts first.** Recommended: the oldest runs server-wide, from the logs keeping the
  most, never below 20 runs a deployment; the backup first when there is one.
- **Q5. Backups in the first cut**: a folder only (it covers external drives and synced cloud
  folders); object storage next; Holon Cloud named as coming.
- **Q6. Default thresholds**: warn at 85% in use, act at 95%.
