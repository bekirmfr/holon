// Hit counter: writes that run with their route.
//
// A route counts its hits though nothing in it reads the request: a write in a route is tied to
// that route's request, so each request makes one. Another route puts a visit in a collection and
// records the visitor only once the put has answered (`if (saved !== undefined)`), so
// `state.last` never names a visit that isn't kept.
//
// Try it: deploy it (holon deploy counter.holon.ts), then
//   curl http://127.0.0.1:8155/hit                     "hit 1", then "hit 2"
//   curl -X POST http://127.0.0.1:8155/visits/ada      "visited ada"
//   curl http://127.0.0.1:8155/last                    "ada"
import { flow, route } from "holon"
import state from "state"
import visits from "collection:visits"
import { now } from "holon:clock"

flow("counter", { port: 8155 })

// Each hit adds one to the count kept across runs, and says it: ++state.hits is the count it
// makes, from 0 when there is none yet.
route.get("/hit", () => `hit ${++state.hits}`)

// Keeps when the visitor came, then (after the put) remembers who came last.
route.post("/visits/:id", (req) => {
  const id = req.params.id
  const saved = visits.put(id, now)
  if (saved !== undefined) state.last = id
  return `visited ${id}`
})

// Who visited last, or "nobody yet".
route.get("/last", () => state.last ?? "nobody yet")
