// Pulse: a clock that beats, and a route that answers 503 for a beat that hasn't come yet.
//
// Every 5 seconds it beats: one more in the count, and the time of this one. GET /beat/:n answers
// the nth beat once it has come, and 503 (unavailable, for now) before: the runs that answer 503
// are runs that failed. The Overview counts both hour by hour from what each log keeps of its
// runs: its flows table shows a bar an hour, red where runs failed, and its tiles how many ran and
// failed in the last 24 hours.
//
// Try it: deploy it, wait a little, then
//   curl http://127.0.0.1:8183/beat/1                   {"beat":1,"of":…}
//   curl http://127.0.0.1:8183/beat/100000              503
// and open the Overview (or the holon server's GET /summary?hours=1, with your token).
import { every, flow, route, status } from "holon"
import state from "state"
import { now } from "holon:clock"

flow("pulse", { port: 8183 })

// Each beat: counted and timed.
every("5s", () => {
  state.beats = (state.beats ?? 0) + 1
  state.last_beat = now
})

// The nth beat, once it has come; before, 503.
route.get("/beat/:n", (req) => {
  const n = Number(req.params.n)
  const beats = state.beats ?? 0
  return (
    (n >= 1 && n <= beats ? { beat: n, of: beats, last_beat: state.last_beat } : undefined) ??
    status(503)
  )
})
