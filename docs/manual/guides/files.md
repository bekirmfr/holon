# Files on the host

A flow can read and write files on the server's machine, but only in folders an admin has bound
to a name, and only by that name. The flow says `folder inbox`; the admin decides that `inbox` is
`/srv/drop`, read only. The flow never sees a path, and can't reach anything outside the folders
it names.

## Binding a folder

An admin binds folders from the editor's **admin** page (**folders**), or on the server's
machine:

```sh
holon folder add inbox /srv/drop --read-only
holon folder add reports /srv/reports
holon folder list
holon folder remove reports
```

| Option | |
|---|---|
| `--read-only` | flows may read it, not write it |
| `--for SLUG` | for that deployment only: it sees this folder as the name, and no other deployment sees it |
| `--keep hash` | the run log keeps a hash of what each run read, not the text |
| `--keep 30d` | the log keeps the text for 30 days (`s`, `m`, `h` or `d`), then only its hash |
| `--cap 10M` | a write that would take the folder's files past 10 MiB in all doesn't land |

- A name is letters, digits, `-` and `_`, up to 64 characters.
- `--for` lets two deployments use the same name for different folders: bind `inbox` for `acme`
  to `/srv/acme`, and for `globex` to `/srv/globex`.
- The server's own data folder can't be bound.
- Removing a binding leaves the files where they are; deployments that use it stop until it is
  bound again.

A deploy is refused until each folder the flow names is bound, and bound read-write where the
flow writes.

## Reading and writing

A folder reads and writes like a collection, by file name:

```holon
flow inbox-report
port 8112
folder inbox
folder reports

route POST /report (req) {
  name = req.json.name
  text = inbox[name]
  reports["{name}.md"] = "# {name}\n\n{length(split(text, "\n"))} lines"
  respond { report: "{name}.md" }
}

route GET /reports (req) {
  respond reports.list()
}

route DELETE /reports/:name (req) {
  delete reports[req.params.name]
  respond 204
}
```

| Written | Does | Asks for |
|---|---|---|
| `inbox[name]` | the file's text, as the run began; no value if there is none | read files in the folder "inbox" |
| `reports[name] = text` | write the whole file when the run finishes | write, copy into and delete files in the folder "reports" |
| `delete reports[name]` | delete it when the run finishes | the same |
| `reports.list(prefix: p, after: a, limit: n)` | a page of files: name to `{size, hash, modified}`, in name order | read files |

- A name is a path inside the folder (`2026/10/a.csv`), and never leaves it.
- A file read whole is text (UTF-8), up to 1 MiB; a run reads 64 MiB in all at most. Reading
  something that isn't text fails the run, and it writes nothing.
- Writes land together when the run finishes, with the run's state and collection writes, or not
  at all. A write that would take a capped folder past its cap doesn't land, and the run answers
  500.

## Big files

A file of any size is used without it entering the run whole:

```holon
flow log-archive
port 8124
folder archive
folder logs

route GET /logs/:name/tail (req) {
  respond logs.lines(req.params.name, from: -20, limit: 20)
}

route GET /logs/:name/head (req) {
  respond logs.slice(req.params.name, offset: 0, length: 256)
}

route POST /archive (req) {
  log = logs.file(req.json.name)
  archive["{log.name}.{log.hash}"] = log
  respond { archived: "{log.name}.{log.hash}", size: log.size }
}
```

| Written | Gives |
|---|---|
| `logs.file(name)` | a **handle**: `{folder, name, size, hash, modified}`, without the bytes |
| `logs.lines(name, from: i, limit: n)` | `n` lines from line `i` (from the end when negative), without line endings |
| `logs.slice(name, offset: o, length: n)` | `n` bytes from `o` (from the end when negative), as text |
| `archive[n] = handle`, `archive.copy(n, handle)` | copy the file when the run finishes, on the host; it fails if the file no longer has the handle's hash |

Only the lines or bytes read enter the run, and the log keeps with them the hashes that prove
them part of the file, so the run can still be checked offline.

## A run per new file

`on file NAME`, in place of `port` or `every`, makes a flow run once for each new file in that
folder. In each run, `ingress` is the file's handle:

```holon
flow drop-box
on file drop
folder receipts

sent = http.put("httpbin.org", "/put",
  headers: { "content-type": "application/octet-stream" },
  file: ingress)
receipts["{ingress.name}.json"] = json({ file: ingress.name, size: ingress.size, status: sent.status })
```

It asks to "run on each file new, changed or removed in the folder "drop" (as its on file lines say), and pass it on": that is its trigger.

- A file is new once it stops changing: the server looks at the folder every second, and a file
  with the same size and time at two looks in a row starts a run.
- Files already in the folder when the flow is first deployed aren't new.
- Files that arrive while the server is down, or the deployment paused, start their runs when it
  is back.
- A file the flow's own runs wrote into the folder doesn't start it again, unless it changes.
- One `on file` per flow, and only in a flow without routes. A run on a file has the clock run's
  time budget.

## Sending a file

`file:` in place of `body:` sends a file as a request's body, by handle:

```
http.put("files.example.com", "/upload/{log.name}", file: log)
```

The server streams the bytes from the folder; they never pass through the flow, so a file of any
size can be sent. Before and as it sends, it checks they still have the handle's hash; if the file
changed, the call answers `{status: 0, error: "changed"}`. The run log keeps the request with the
handle, never the bytes. The file must be in a folder the flow reads, or be the new file that
started the run. Any method works: `http.post(…, file: h)` too.

## What the run log keeps

Replaying a run needs what it read, so the log keeps it:

- a text up to 4 KiB, in the run's entry;
- a bigger one beside the log, by its hash, in chunks: a file that grows adds only its new
  chunks, not a new copy;
- for a folder bound `--keep hash`, only the hash. `holon verify --files DIR` then checks those
  runs against the original files;
- for `--keep 30d`, the text for 30 days, then only the hash.

What a run wrote is kept by its hash. The server sweeps, every ten minutes, the texts no kept run
refers to; `holon folder sweep` does it at once.

`holon export` takes `--max-files-bytes 10M` to ship at most that much of those texts with the
runs (the oldest runs' first), or `--without-files` for none; it says what it left out. See
[Checking runs](checking-runs.md).
