// Heartbeat: a clock flow.
//
// Every 10 seconds it records the time, counts the beats in its state, and keeps the time of the
// first beat. It has no routes: the clock is all that starts it.
//
// Try it: deploy it, wait a little, then look at its state on the deployed page (or the holon
// server's GET /deployments/heartbeat/state, with your token).
import { every, flow } from "holon"
import state from "state"
import { now } from "holon:clock"

flow("heartbeat")

// Each beat: the time of this one, one more in the count, and the first beat's time, kept once set.
every("10s", () => {
  const t = now
  state.last_beat = t
  state.beats = (state.beats ?? 0) + 1
  state.first_beat = state.first_beat ?? t
})
