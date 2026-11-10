# Logs: a flow writes lines, the host keeps a few, apart from its runs (L4)

How a flow says something while it runs, and where that goes. Written 2026-11-10, before the code.

The aim, in one example: a payout flow writes `log.info("paying", order.id)` and
`log.warn("retry", n)`. The lines are not part of any run's record, so a run's evidence is the
same with or without them. `holon logs payouts --follow` shows them as they come; the console's
Logs panel shows them beside the runs, and with a run selected, the lines up to it.

## 1. Where it stands

- A flow cannot write a line. `console` is one of the compiler's banned globals (`ts.rs`
  `GLOBALS`): `console.log("x")` stops with "console isn't there: a flow has no globals; import
  what it uses".
- What a flow leaves is its **run**: the record each run adds to the deployment's log, chained,
  checked offline by `holon verify`, and the evidence people export. It is the wrong place for
  debug output: it would fill the record, and it would dilute what the chain vouches for.
- There is no `holon logs` command. A deployment's other files live in folders beside the
  deployments file (`Place::beside`: state, mail-seen, feed-seen, queue-seen, the runs' log); a
  server that keeps nothing (`Place::Nowhere`) keeps them in memory.
- A handler's `log` option (`strict`, `lazy`, `unrecorded`) is about its run's record, and stays
  about that. This document's logs are a second thing and share only the word.

## 2. Goals and non-goals

Goals:

1. **A flow can write a line**, with a level, in a way that reads like the logging people know.
2. **Lines are kept apart from runs**: not in a run's record, not in the chain, not in evidence,
   so replay and `holon verify` give the same answer with or without them.
3. **Bounded.** A loud flow loses its oldest lines; it never fills the disk or slows its answers.
4. **Readable afterwards**: after a restart or a crash, which is when they are needed.
5. **Shown beside the runs**: a command, and a panel in the console and in `holon editor`.
6. **Never a way to leak**: a secret's value is not written.

Non-goals here: shipping lines to a collector (OpenTelemetry, Seq); searching across
deployments; structured queries over fields; alerting on a line; keeping logs as evidence.

Dev phase: no compatibility. `console` stays banned as a global; the `console.log` spelling (§3)
is a name the compiler turns into the leaf, not a global that exists.

## 3. The leaf

`log` is imported from `holon`, like every leaf a flow uses:

```ts
import { log } from "holon"

log.info("paying", order.id)
log.warn("retry", n)
log.error("refused", reason)
```

Levels are `debug`, `info`, `warn` and `error`; the arguments are any values, written in their
JSON form, joined by a space. A line's text is capped (a long value is cut, and the line says so).

`console.log(…)`, `console.warn(…)` and `console.error(…)` compile to the same node (`info`,
`warn`, `error`), so a person who types them gets a line, not the "no globals" error. `console`
is still not a value: `const c = console` stays an error. (Open question 1.)

The node is a driver with no outputs, as `random` is a driver with one. It adds no input to any
other node, so it changes nothing a flow answers. When a run is replayed or verified, the node
does nothing: it writes no line and its output is not part of the comparison.

## 4. Where lines go

One **file per deployment**, not per version, in a `logs` folder beside the deployments file:
`logs/<slug>.jsonl`, one line of JSON each:

```json
{"at": 1794257758123, "level": "warn", "version": "c59829c4", "run": 17, "text": "retry 3"}
```

- `run` is the run's sequence number, so a line can be put beside its run without being in it.
- **Rolling, with a hard cap.** When `<slug>.jsonl` reaches the size cap it becomes
  `<slug>.1.jsonl`, and the oldest of the kept files is deleted. Defaults: 3 files of 2 MB.
  A cut-back is counted: the next line says how many were lost (`{"level":"info","dropped":412}`).
- **Written lazily.** Lines are buffered and written about once a second, on a writer of their
  own: the same bargain as `lazy` runs, a crash can lose the last second. A run never waits for
  its lines. A line that cannot be written is counted and dropped; the flow goes on.
- **Capped per deployment.** A line over its size cap is cut; a deployment over its rate cap
  (default 1,000 lines a second) drops the excess and counts it, so one loop cannot use up the
  disk's writes.
- **A server that keeps nothing** keeps the same lines in a memory ring of the same size.
- **Removed with the deployment**: `holon remove` deletes `logs/<slug>*`, as it deletes the
  deployment's state.
- **Stdout.** `holon serve --log-stdout` also writes each line to the server's stdout, for
  someone who runs it under systemd or a container that collects it. Off by default.

A secret's value is not written. Where a line's arguments are the result of a secret read, or
computed from one, the compiler refuses the call ("a secret's value may not go in a log").
(Open question 3.)

## 5. Reading them

- `GET /deployments/<slug>/logs?level=warn&run=17&until=17&after=<at>&limit=500`: lines, oldest
  first, from the files. `run=17` is that run's lines; `until=17` is every line up to the end of
  run 17. Needs the viewer's access to the deployment, as its runs do.
- `holon logs <slug> [--level warn] [--run N | --until N] [--follow] [--limit N]`: the same,
  and `--follow` keeps reading as the file grows.
- **The console's Logs panel**, a terminal-style pane beside the runs: every line by default,
  following; with a run selected, it shows the lines up to that run, and a toggle narrows it to
  that run's own. Level filter, and a line for "N lines were dropped here".
- **`holon editor`** has the same panel in its own tabs.

## 6. Permission

None. The node writes only to the host's own logs folder; it reaches nothing outside the host,
so it is no capability a person approves at deploy. The deploy preview does not list it. What
leaves the host (the stdout flag) is the host owner's choice. (Open question 2.)

## 7. Samples and tests

- **Sample `traced-payout`**: a flow that logs each stage of a payout at its own level, with a
  route that answers and a page that shows the last lines. Run live once before the commit.
- Tests: the compiler turns `console.log` into the leaf and refuses `const c = console`; a run
  with logs and the same run without give the same record and the same evidence hash; replay
  writes no line; the file rolls at its cap and counts what it dropped; a line over its caps is
  cut or dropped, not an error to the flow; a secret read into a log call is refused; `holon
  remove` deletes the files; `holon logs --follow` shows a new line within two seconds.

## 8. Steps

1. **The leaf**: `log.info/warn/error/debug` and the `console` spellings, lowered to a driver
   that changes nothing; replay does nothing for it; catalog, manual leaves and `holon.d.ts`
   made again; the sample.
2. **The sink**: the per-deployment file, rolling, lazy writer, caps, the in-memory fallback,
   removal with the deployment.
3. **Reading**: the route and `holon logs`.
4. **The console panel** and the panel in `holon editor`.
5. **`--log-stdout`** and the settings for the caps.

Steps 1 and 2 land together so the first lines go somewhere.

## 9. Open questions

1. **Is `console.log` accepted?** It is what people type, and it costs one rule in the compiler.
   The cost is that `console` becomes a name that exists. Leaning: accept the three spellings,
   nothing else on `console`.
2. **Does `log` show in the deploy preview?** Not as a permission. It might show as a quiet line
   ("writes logs") so a reader sees it. Leaning: show it, with no approval.
3. **How is a secret kept out?** A compile-time check that no argument derives from a secret
   read is the strict answer; a run-time redaction of known secret values is the cheaper one and
   misses a value that is transformed. Leaning: the compile-time check, as far as the data flow
   can follow.
4. **The caps.** 3 files of 2 MB and 1,000 lines a second are guesses. Are they host settings, or
   a flow's own (`flow("x", { logs: … })`)? Leaning: host settings first.
5. **Does a line outlive a redeploy?** Leaning: yes; the file is the deployment's, and each line
   names its version.
