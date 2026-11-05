// Hit counter (samples/counter.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"
import state from "state"
import visits from "collection:visits"
import { now } from "holon:clock"

flow("counter", { port: 8155 })

route.get("/hit", () => {
  const hits = (state.hits ?? 0) + 1
  state.hits = hits
  return `hit ${hits}`
})

// The visitor is recorded once the visit is kept: the write waits for the put.
route.post("/visits/:id", async (req) => {
  const id = req.params.id
  const saved = await visits.put(id, now)
  if (saved !== undefined) state.last = id
  return `visited ${id}`
})

route.get("/last", () => state.last ?? "nobody yet")
