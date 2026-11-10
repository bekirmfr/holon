# Logs: a flow writes lines, the host keeps a few, apart from its runs (L4)

How a flow says something while it runs, and where that goes. Written 2026-11-10, before the code;
steps 1 to 3 of §8 are built, and this text says what they do.

The aim, in one example: a payout flow writes `console.log("paying", order.id)` and
`console.warn("retry", n)`. The lines are no effect a run performed, so `holon verify` checks the run
without them. `holon logs payouts --follow` shows them as they come; the console's
Logs panel (§8, step 4) shows them beside the runs, and with a run selected, the lines up to it.

## 1. Where it stood

- A flow could not write a line. `console` was one of the compiler's banned globals: `console.log("x")`
  stopped with "console isn't there: a flow has no globals; import what it uses".
- What a flow leaves is its **run**: the record each run adds to the deployment's log, chained,
  checked offline by `holon verify`, and the evidence people export. It is the wrong place for
  debug output: it would fill the record, and it would dilute what the chain vouches for.
- A deployment's other files live in folders beside the deployments file (`Place::beside`: state,
  mail-seen, feed-seen, queue-seen); a server that keeps nothing (`Place::Nowhere`) keeps them in
  memory.
- A handler's `log` option (`strict`, `lazy`, `unrecorded`) is about its run's record, and stays
  about that. This document's logs are a second thing and share only the word.

## 2. Goals and non-goals

Goals:

1. **A flow can write a line**, with a level, in the standard library's own grammar: `console.log`.
2. **Lines are kept apart from runs**: no effect a run performs, so not in its performed list, not in
   the chain, not in evidence, and replay and `holon verify` leave them out.
3. **Bounded.** A loud flow loses its oldest lines; it never fills the disk or slows its answers.
4. **Readable afterwards**: after a restart or a crash, which is when they are needed.
5. **Shown beside the runs**: a command, and a panel in the console and in `holon editor`.
6. **Reading is a person's, and an agent's only by a grant** (§6).
7. **Never a way to leak**: a secret's value is not written.

Non-goals here: shipping lines to a collector (OpenTelemetry, Seq); searching across
deployments; structured queries over fields; alerting on a line; keeping logs as evidence; a flow
reading its own lines back (that would be an input no run records, and replay would break).

Dev phase: no compatibility. `console` is still not a value (`const c = console` is an error);
`console.log(…)` and its four siblings are calls the compiler turns into the leaf.

## 3. The leaf

Everything in a flow's text reads as the standard library, so a line is written the way every
programmer writes one, with no import:

```ts
console.log("paying", order.id)
console.info("queued")
console.debug("checking", order.id)
console.warn("retry", n)
console.error("refused", reason)
```

The method is the level: `debug`, `log`, `info`, `warn`, `error` (`log` and `info` rank the same
when a reader filters by level). Several arguments are one text, written as the template
`paying ${order.id}`, so a flow's text has one form for a line; a text is written as it is, any
other value as JSON. A line's text is cut at 4 KiB, and says so. Any other method (`console.table`)
is an error naming the five.

Underneath it is a leaf, `log`, with a `level` param and one input: a driver with no output that
adds no input to any other node, so it changes nothing a flow answers. Its capability is
`Log("warn")` and so on: named in what a flow may do ("write warn lines to its log, which is kept
apart from its runs"), but never an item a person approves at deploy (§6).

When a run performs its effects, the host that records them sets a log line aside instead of
adding it to `performed`; replay and `holon verify` use the same host, so what they compare leaves the
lines out, and a replay writes none. What the run computed to write a line (a template's text) is in its
record as any value is.

## 4. Where lines go

One **file per deployment**, not per version, in a folder beside the deployments file
(`<deployments>.logs/<slug>.jsonl`), a line of JSON each:

```json
{"at": 1794257758123, "level": "warn", "unit": "c59829c4", "run": 17, "text": "retry 3"}
```

- `run` is the run's seq in the deployment's log, so a line can be put beside its run without being
  in it. `unit` is the first 8 hex of the version that wrote it. `at` is in ms and rises with each
  line a deployment writes (a line's time is never earlier than the one before's), so a reader
  that has the last one it saw reads on from there and misses none.
- **Rolling, with a hard cap.** At 2 MB `<slug>.jsonl` becomes `<slug>.1.jsonl` and then
  `<slug>.2.jsonl`; the oldest of the three is deleted. The size and the number of files are the
  server's settings (`--log-size`, default 2M and at least 64K; `--log-files`, 1 to 10, default 3).
- **Written lazily.** Lines are buffered and written about once a second, on a thread of their own:
  a run never waits for its lines, and a crash can lose the last second. A file that can't be
  written counts its lines as dropped.
- **Capped.** A line over 4 KiB is cut; a deployment writing over 1,000 lines a second (`--log-rate`)
  drops the excess and the next line says how many (`"3 lines were dropped: over 1000 a second"`, with
  `dropped: 3`).
- **A server that keeps nothing** keeps the newest 5,000 lines of each deployment in memory.
- **Stdout.** `holon serve --log-stdout` (or `HOLON_LOG_STDOUT=1`) also prints each line as it is kept,
  on one line, from the thread that writes the files, so a run never waits on a full pipe:
  `11:48:03.678 traced-payout: LOG   run 0  payout asked p-1 250` (UTC, the deployment named, a
  newline in the text written as `\n`). It works with no folder too. Off by default.
- **Removed with the deployment**: removing it deletes its files.
- **Secrets.** A line computed from a secret, its value in it as it is or encoded, is withheld
  (`‹withheld: computed from a secret›`), as the record withholds an effect's payload computed from
  one; the run's own redactor then writes the value of any secret the run holds as a mark, for what
  taint misses. The taint is per level: if a run's `log` lines derive from a secret, all its `log`
  lines are withheld. (Open question 3.)
- A run of an `unrecorded` handler writes no lines.

## 5. Reading them

- `GET /deployments/<slug>/logs?level=warn&run=17&until=17&after=<at>&limit=200`: lines, oldest
  first. `run=17` is that run's lines; `until=17` is every line up to the end of run 17's.
- `holon logs <slug> [--level warn] [--run N | --until N] [--limit N] [--follow] [--json]`: the same;
  `--follow` keeps reading as the file grows.
- **The console's Logs panel** (step 4, built): a terminal-style pane on a flow's page, in a Logs
  tab beside Runs: every line, following as they come (or paused), with a level filter (all,
  warnings, errors); a line's run is a link to the run. A run's page has a Log lines section: the
  lines up to the end of that run, and a toggle narrows it to the run's own. A dropped line shows
  dim, as "N lines were dropped". `holon editor` has the same in its flow page's Logs section (built): a list of lines, oldest first,
  the newest picked as they come, with the picked line's detail beside it; `w` the level, `u` a run's
  lines (up to its end, then its own, then every run's), `F` pause, enter to step through the run, and
  `L` on a run in the Runs section opens the lines up to it.

## 6. Permission

**Writing** needs none. A line reaches only the host's own logs folder, nothing outside it, so it
is not asked for at deploy, though the deploy preview says the flow writes lines.

**Reading** is a person's: anyone signed in who may read the deployment reads its lines through the
console and `holon logs`, as they read its runs. **An agent**, `holon mcp` and anything that asks
as an agent, **reads them only by a grant**: lines hold what a flow chose to say, and an agent
reading a deployment's runs is not an agent reading its logs. Without the grant the route refuses an
agent (403), and says who lets it.

The grant is the deployer's to ask for and a person's to approve, as `agents may run <action>` is
(`AgentRuns`): the requirement `agent_logs` in the deploy settings (a box, **agents may read its logs**, for a
flow that writes lines) makes one manifest item, `AgentReadsLogs`, which the deploy preview has a person approve
("let agents read its log lines"), optional (the flow runs without it), held per deployment and taken back like
any grant. As `AgentRuns` is, it is asked for in the deploy settings only: the CLI and `holon mcp` deploy from
the flow's text, which can't declare it. `holon mcp` has a `logs` tool (`slug`, `level`, `run`, `own`, `limit`)
that reads through the same route, so the same grant answers it.

## 7. Samples and tests

- **Sample `traced-payout`**: a payout route that writes `console.log`, `console.debug` and, over a
  limit, `console.warn`, and refuses the large one. Tried live through the real server in
  `editor/e2e/cli.e2e.ts`: deployed from its text, two payouts sent, the lines read with `holon logs`
  (all, `--level warn`, `--run 0`), the deployment's own log shown to hold none, an agent refused,
  and the lines gone with the deployment.
- Tests: the compiler turns `console.log` and the others into the leaf and prints it back as
  written (`print.rs`); a flow's lines are in the sink with their run and level, the run's record
  holds no line and no `Log(…)`, and the deploy asks for no approval to write one (`tests.rs`); the
  sink reads back oldest first, filters by level, run and time, cuts a long line, drops over its
  rate and says how many, rolls at its cap and keeps three files, survives a restart, and refuses a
  name that is a path (`logs.rs`).

## 8. Steps

1. **The leaf**: `console.*` lowered to the `log` leaf, its capability, the printer, `holon.d.ts`,
   the manual's leaves; done.
2. **The sink**: the per-deployment file, rolling, lazy writer, caps, the in-memory fallback,
   removal with the deployment; done.
3. **Reading**: the route and `holon logs`, an agent refused; the sample `traced-payout`; done.
4. **The console panel** (the Logs tab and a run's Log lines) and the panel in `holon editor` (its Logs
   section); done.
5. **`holon serve --log-stdout`** and the settings for the caps (`--log-size`, `--log-files`,
   `--log-rate`, and `HOLON_LOG_*`); done.
6. **The agent's grant**: the manifest item `AgentReadsLogs`, the deploy settings' box, the route's check and
   the `holon mcp` `logs` tool; done.

## 9. Open questions

1. ~~**Is `console.log` accepted?**~~ Yes, and it is the only spelling: the code editor reads as the
   standard library.
2. **Does `log` show in the deploy preview?** It shows in what a flow may do ("write warn lines to
   its log"); it is never an item to approve. Leaning: keep it so.
3. **How is a secret kept out?** Built: a line computed from a secret is withheld, by the run's
   taint (per level, so one such `log` line withholds the run's others at that level), and the
   redactor writes a secret's value as a mark for what taint misses. Whether to refuse the call at
   compile time instead, as a secret into a request to a host that isn't granted it is refused, is
   open.
4. ~~**The grant's shape.**~~ One item per deployment, not per level: `AgentReadsLogs`; and `holon mcp`'s `logs`
   tool reads through the same route, so one check serves both.
5. ~~**The caps.**~~ The server's settings, by flags and `HOLON_LOG_*` variables, with the old
   constants as their defaults; a flow's own caps are not planned.
6. **Does a line outlive a redeploy?** Yes, built so: the file is the deployment's, and each line
   names its version.
