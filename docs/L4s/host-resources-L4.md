# Host resources: one address for what flows touch, and who touched it when (L4)

**Status:** design, before any code. Written 2026-10-25.

A host runs many flows (a thousand, say), and they touch the same things: a folder a partner
drops files into, an outside API, a file one flow writes and another reads. Each run's log says
what that run touched, but each log is one deployment's. This note gives everything a flow can
touch an **address that is the same across the host**, and keeps a **timeline per resource**:
which deployments read, wrote, sent or were started by it, in time order, each row pointing to the
run that did it, and each row checkable against that run's log.

```
$ holon resources timeline folder:receipts/ --since 1d
2026-10-25 20:11:35.912  drop-box        write   receipts/sales-live.csv.json  138 B  d014e15d…  #0
2026-10-25 20:11:36.204  receipt-ledger  arrive  receipts/sales-live.csv.json  138 B  d014e15d…  #0
2026-10-25 20:11:36.231  receipt-ledger  read    receipts/sales-live.csv.json  138 B  d014e15d…  #0
```

The flow's text doesn't change: a flow still names `folder receipts`, and the host decides which
resource that is.

---

## 1. What is already there

- **Addressed capabilities.** A flow's manifest names what it touches, derived from its graph:
  `FileRead("inbox")`, `FileWrite("reports")`, `FileArrival("drop")`, `CollectionRead("notes")`,
  `CollectionWrite("notes")`, `StateRead(key)`, `StateWrite(key)`, `SecretRead(name)`,
  `HttpRequest(host)` (`holon-core/src/kind.rs`). The grant records them (`permissions.rs`) and
  can be changed while deployed (`grants.rs`).
- **But the names are the flow's.** What a name *is* depends on the resource kind:
  - **Folders** are already host things: an admin binds a name to a path, for the whole server
    or for one deployment (`folders.rs`, `Bound { server, deployments }`). Two deployments
    that both say `drop` share the server's binding unless one has its own.
  - **Collections and state** belong to a deployment: they live in its state (`#collection:<name>`
    cells, `collections.rs`), so `orders` in one flow and `orders` in another are different data.
  - **Secrets** belong to a deployment too: each has its own vault (`secrets.rs`). A value may
    come from a reference (`secret_sources.rs`: an environment variable, a Vault path), which two
    deployments can share without anything saying so.
  - **Outside hosts** already have a global name: the host (`HttpRequest("httpbin.org")`).
- **What a run touched is in its log entry** (`recorder.rs`, `LogEntry`):
  - `reads`: collection reads, and file reads with their hash and size (`LoggedFile`);
  - `performed`: the effects performed, by capability and payload (a file write as
    `{name, size, hash}`);
  - `rounds`: the outbound requests, each with its URL and its start and end times;
  - `sourced`: what it was handed (a request, a new file's handle under `FileArrival`).

  Secrets are never in it: a run's secrets aren't part of its input (`firing.rs`), and a replay
  stands a placeholder in.
- **Logs are per deployment and short.** Each keeps its newest 500 runs (`DEFAULT_KEEP`, or
  `--keep-runs`) in `<deployments>.logs/<slug>.jsonl`, a hash chain (each line's `prev` is the
  BLAKE3 of the line before), checked when opened. `holon export` / `holon verify` check one
  deployment's runs offline.
- **The activity log** (`activity.rs`) is the control plane's: saves, deploys, grants, folder
  bindings. It is not about runs, and stays that way. A file write in it, as named-folders §9
  planned, isn't needed: what runs did is what this note indexes.

## 2. Two kinds of resource

| Kind | Who it belongs to | Its address | Defined where |
|---|---|---|---|
| folder | the host | `folder:<id>` | the host's resources (§3) |
| file | its folder | `folder:<id>/<relative name>` | follows from the folder |
| outside host | the world | `http:<host>` | nothing to define: the host name |
| outside endpoint | its host | `http:<host>/<path>` | follows from the host |
| collection | a deployment | `collection:<slug>/<name>` | the deployment |
| item | its collection | `collection:<slug>/<name>/<key>` | follows from the collection |
| secret | a deployment | `secret:<slug>/<name>` | the deployment's vault |

- **Shared resources** (folders, outside hosts) are the ones several deployments touch, so they
  are the ones the timeline is for across flows. Only folders need defining on the host:
  an outside host's name is already global.
- **Owned resources** (a deployment's collections, state and secrets) are unique on the host
  already, as `<slug>/<name>`. They need no registry. They're in the timeline because a resource's
  timeline should be one place to look whatever its kind, and because "who touched this customer's
  item" is asked of collections too.
- **Sharing a collection or a secret** between deployments is a different change: a store
  outside any one deployment's state, with claims across deployments, or a secret whose source
  is a host resource. That is its own note (§12, fork 8). Nothing here stops it: it would add
  kinds the host defines, as folders are.

## 3. Folders become host resources

Today a folder binding is a name and a path; the name is both what flows say and what the host
calls it. This splits the two.

**A host resource** is `{id, name, kind, path, access, keep, cap}`. An admin defines it:

```json
{ "resources": {
    "r_7f3a9c": { "name": "partner-drop", "kind": "folder", "path": "D:\\partner\\in",
                  "access": "read", "keep": "bytes" },
    "r_0b41de": { "name": "receipts", "kind": "folder", "path": "D:\\receipts",
                  "access": "read-write", "keep": "hash" } },
  "bindings": {
    "@server":  { "receipts": "r_0b41de" },
    "drop-box": { "drop": "r_7f3a9c" } } }
```

- **The id is stable** (`r_` and 6 bytes, random, made once) and is what every address and
  every timeline row uses. The **name** is for people and can change; renaming is recorded in
  the activity log and changes no row. **Moving** the path keeps the id, so the history stays
  one history.
- **One path, one resource.** Two resources may not have the same real path, or one inside the
  other (checked when defined, links resolved, as binding checks the server's own data today).
  Otherwise a file written through one name and read through another would show as two
  unrelated files, and a timeline of the outer folder would miss what was done through the
  inner one. To give several deployments the same folder, bind each to the one resource.
- **Bindings map a flow's name to a resource,** for the whole server (`@server`) or for one
  deployment, exactly as today's two levels: a deployment's own binding of a name first, then
  the server's.
- **The grant names the resource,** next to the flow's name for it:
  `FileWrite("receipts") → r_0b41de`. Rebinding a deployment's name to another resource is a
  grant change (`grants.rs`): the deployment is re-checked and its permit rebuilt, and a binding
  to a resource it wasn't granted waits for approval like any new permission.
- **Migration.** Each server binding becomes a resource of the same name, bound under
  `@server`. Each deployment's own binding becomes a resource named `<slug>.<name>`, bound for
  that deployment. Two bindings with the same path become one resource, and nested paths are
  reported (the server starts; such a binding can't be changed until the overlap is resolved).
  `--folders` keeps its file, in the new shape; `holon folder add` keeps working (it defines a
  resource and binds it in one step).

## 4. Addresses

An address is `<kind>:<scope>[/<path>]`, compared byte for byte:

- **folder, file:** `folder:<id>/<name>`, the name relative and confined as a flow's is
  (`/`-separated, Windows names case-folded as the folder compares them). Shown to people with
  the resource's current name (`receipts/sales-live.csv.json`); kept with the id.
- **http:** `http:<host>[:<port>]/<path>`: the host lowercased, a default port dropped, the path
  as sent, **without its query or fragment** (they carry ids and tokens more often than paths
  do). A query is never indexed.
- **collection, item:** `collection:<slug>/<name>/<key>`, the key as the flow wrote it.
- **secret:** `secret:<slug>/<name>`. Never a value.

A **prefix** names everything under it: `folder:r_0b41de/` is the folder, `http:api.stripe.com/`
the host, `collection:shop/orders/` the collection. A query may name a folder by its current
name (`folder:receipts/`); the host resolves it to the id before it looks.

## 5. The timeline

**A row** is one thing a run did to one resource:

```json
{ "at_us": 1792948295912000, "address": "folder:r_0b41de/sales-live.csv.json", "verb": "write",
  "slug": "drop-box", "seq": 0, "run": "b4840571…", "entry": "73bae10a…",
  "hash": "d014e15d…", "size": 138 }
```

- **Verbs:** `read` and `list` (a run read it, or listed under it), `arrive` (it started the
  run), `write`, `copy` (written from a handle; the row carries the source's address too),
  `delete`, `send` (its bytes were a request's body), `call` (an outside endpoint was called;
  the row carries the method and status), `put` (an item written), and `secret` (a secret read).
- **Derived from the log entry**, when the entry is logged, never from anything else:
  `reads` give read and list rows (files with their hash), `performed` gives write, copy, delete
  and put, `rounds` gives call and send, and `sourced` gives arrive. A secret's row comes from
  the run's timeline (a `secret` node that ran), with the name only: the log never holds the
  value, and the index doesn't either.
- **Its time is when it happened,** as far as the run says: a run reads as it began (`at_ms`, a
  snapshot), its writes land when it finishes (`at_ms + duration_ms`), and a call is when it was
  sent (`start_us` of its exchange). Rows from different deployments are ordered by that, then by
  slug and seq: one host, one clock.
- **`entry` is the BLAKE3 of the run's log line,** the hash the next line's `prev` holds. That
  makes a row provable: it names a line of a hash chain, and that line, re-derived, must give
  the row (§8).
- **Rows never carry values:** addresses, sizes, hashes, a method and status. A row says *that*
  `collection:shop/orders/123` was written, not with what.

**Where rows are kept.** In `<deployments>.resources/`, one append-only file of JSON lines per
day (`2026-10-25.jsonl`), owner-only, written beside the log line. When a day closes it gets a
small sidecar, an index from address and hash to line offsets, so a query on a closed day reads
only its matching lines. The open day is scanned.

**They outlive the logs.** A log keeps 500 runs; the timeline keeps a year by default
(`--keep-resources 400d`). A row whose run has been cut from its log says so ("run no longer
kept"): it still says who touched what and when, and it can be checked only if the run was
exported before it was cut. Deleting a deployment keeps its rows, marked deleted, as the
activity log keeps that it was.

**Rebuilt from the logs.** `holon resources rebuild` derives the rows of every run still kept
again, and keeps the rows of runs no longer kept. A server upgraded to this builds its first
timeline the same way.

**Asked for:**

- `GET /resources/timeline?address=<prefix>&from=&to=&verb=&slug=&limit=&after=`, oldest first,
  paged; `holon resources timeline <address> [--since 7d]`.
- `GET /resources/timeline?hash=<hex>`: every row with those bytes, any resource (§6).
- `GET /resources` lists the host's resources; `GET /resources/<address>` describes one (its kind,
  its name, which deployments are granted it and how, §7).

**Who may ask.** The timeline is run logs, read across deployments: whoever may read deployment
logs (a viewer and up) may read it. A folder's path is shown to admins only, as `/admin/folders`
is today.

## 6. Following content by hash

Every file a run reads, gets handed, writes, copies or sends has a BLAKE3 hash in its log entry,
and so in its rows. A query by hash gives **where those exact bytes were seen**: dropped in
`partner-drop`, sent to `api.partner.com`, copied to `archive`, read by a reporting flow.

- **Exact, not derived.** Bytes a flow transforms (a CSV turned into a report) have a new hash:
  the timeline shows the read and the write, in the same run, but doesn't claim one came from
  the other. A copy and a send carry the same hash, so they always connect.
- Collections' items aren't hashed in the first slices (§12, fork 7).

## 7. Who could, next to who did

The grants say which deployments **may** touch each resource, and how (`FileRead`, `FileWrite`,
`FileArrival`, `HttpRequest`, …), across every deployment on the host. `GET /resources/<address>`
answers both:

- **Granted:** each deployment granted it, with its verbs.
- **Used:** for each, when it last did, from the timeline.
- **Granted and unused** for a while (`--unused 30d`): a permission nothing exercises, a
  candidate to revoke (`grants.rs` already revokes one item).
- **Used and not granted** can't happen (the permit refuses it before it is performed). A row
  whose deployment's grant at the time didn't cover it is reported by `holon verify` as an
  error: the index or the permit is wrong.

## 8. Export and verify

`holon resources export <address> [--since 30d] -o audit.json` writes the rows, and for every
run they name, its deployment's log line and enough of the chain above it to reach the export's
head (or a reference to a `holon export` of that deployment that holds it).
`holon verify audit.json` then checks, offline:

- each row's `entry` is a line of its deployment's chain, unbroken to the head;
- that line, re-derived (§5), gives exactly that row;
- the run, re-run as `holon verify` re-runs any run, produces what its line says;
- the grant it ran under covered the row (§7).

So the answer to "which flows touched this file in October" is evidence, not a report.

## 9. The editor

- A **Resources** page: type an address or paste a hash, see the timeline (time, deployment,
  verb, name, size and hash), newest first; each row opens its run in view mode. A resource's
  header shows its kind, its name, the deployments granted it and how, and when each last used it.
- In view mode, a file name, an outside host or a collection key in a run's log links to its
  timeline.
- The admin panel's folders section becomes resources: define, rename, move, bind.

## 10. The sample

`receipt-ledger` (`crates/holon-author/samples/receipt-ledger.holon`): `on file receipts`, then
it reads each new receipt `drop-box` writes and puts a line for it in a collection `ledger`. It
shows a folder that two deployments share: `drop-box` writes `receipts`, `receipt-ledger`
is started by it and reads it. Its try line: deploy both, bind `receipts` to the same resource,
drop a file in `drop`, then `holon resources timeline folder:receipts/` shows the write, the
arrival and the read, from two deployments, in order. The end-to-end test deploys both and checks
those three rows through the API, then exports them and verifies the export.

## 11. Slices

1. **Folders as host resources.** Resource ids and names, the one-path rule, bindings to
   resources (server and per deployment), grants naming the resource, rebinding as a grant
   change; the migration of today's bindings (`--folders`, the admin panel, `holon folder`);
   `GET /resources`. No timeline yet: this is the identity the timeline keys on.
2. **The timeline.** Rows derived as each run is logged, for folders and files, outside
   endpoints and collection items (secrets once the run's timeline names the leaf), the day
   files, `rebuild`, retention, the timeline query by address prefix (API and CLI), who may ask;
   the sample, with its test and its e2e run.
3. **Hashes and grants.** The query by hash, the closed days' sidecars, `GET
   /resources/<address>` with granted and used, unused grants.
4. **Export and verify.** `holon resources export`, `holon verify` of a timeline: the chain, the
   re-derived rows, the re-runs, the grants.
5. **The editor.** The Resources page, links from view mode, the admin panel's resources.

## 12. Forks, surfaced

1. **A resource's id: opaque, or its name?** Opaque (`r_…`) with a name that can change. A
   name as the id would make a rename split the history, and a name is chosen before anyone
   knows what it'll be called in a year.
2. **Overlapping folders: refuse, or allow and index under each?** Refuse. Allowed, every row
   under the inner folder would need a second row under the outer one, and every binding check
   would have to reason about both; one resource bound to many deployments says the same thing.
3. **Where rows live: day files, or SQLite?** Day files with a sidecar per closed day: append
   only, like the logs, nothing new to run or back up, and a day's query reads only its matches.
   Revisit if a host's day runs to tens of millions of rows.
4. **Retention: the logs', or longer?** Longer (a year by default). An audit trail that forgets
   with a 500-run log is no trail; a row is a hundred-odd bytes.
5. **A deleted deployment's rows: kept or removed?** Kept, marked. The activity log keeps that
   a deployment was; the timeline keeps what it did.
6. **URLs: host and path, or host only?** Host and path, never the query. A path says which API
   was called (`/v1/refunds`), which is the point; the query is where ids and tokens go.
7. **Collection items by hash?** Not at first: an item's value is in the log already, and
   hashing every put is cheap but makes rows for values that are rarely followed. Add it when
   asked.
8. **Shared collections and secrets.** Not here. Sharing a collection means a store outside any
   deployment's state and claims across deployments; sharing a secret means a host resource
   whose value deployments read. Each is its own note, and each adds a kind to §2 without
   changing addresses.
9. **State keys.** Left out: a deployment's state is its own, read whole at every run
   (`state_before`), and a row per key per run would bury the rest.
