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

In `holon.toml`, beside the rest of the host's policy, and editable from the admin page (which
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

1. **`[storage] sweep_every`**, both sweeps, shown on the admin page; `off`.
2. **`GET /admin/host`**, `holon host`, the admin page's panel: memory, disks, what is kept, sweeps.
3. **Per-flow shares**: counted in the sweeps' walks and as entries, states and closures change;
   `GET /deployments/<slug>/holds`; the panel's rows, each flow's page in both editors.
4. **`warn_at`** and the warnings.
5. **`full_at` and `when_full`**: `warn`, `pause` (and resuming), `roll`.
6. **`backup` to a folder**: retention and `roll` export what they cut there first.
7. **`backup` to object storage**; Holon Cloud named as coming.

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
