// Rolling log: retention, only the newest runs kept.
//
// A clock flow that beats every second and keeps only its newest 5 runs, none older than a day:
// its keep option says how long the deployment's log keeps its runs (the deploy settings' keep_runs
// and keep_run_days), and keeping fewer than the host's default asks nobody. Its state still
// counts every beat. The states its cut runs left stay in the store until a sweep clears what
// nothing kept names: the server's own (holon.toml's [storage] sweep_every, ten minutes unless
// said), or an admin's holon store sweep.
//
// Try it: deploy it and wait a few seconds: view mode's runs list holds the newest 5 (or
// holon history rolling-log), their seqs climbing, while the state counts every beat. Then
//   holon store sweep --dry-run      counts the states no kept run names
//   holon store sweep                removes them, an hour after they were put
import { every, flow } from "holon"
import state from "state"
import { now } from "holon:clock"

flow("rolling-log", { keep: { runs: 5, days: 1 } })

// Each beat counts itself and notes its time.
every("1s", () => {
  state.beats = (state.beats ?? 0) + 1
  state.last_beat = now
})
