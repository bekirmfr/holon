// Cache-aside: a branch not taken.
//
// Hands out an id per name, fetched once (a fresh uuid from httpbin.org) and kept in a collection.
// A name it already has an id for is answered from the collection, and the fetch, the fallback it
// doesn't need, isn't run at all, so no request goes out. The run log and view mode show the fetch
// as not run.
//
// Try it: deploy it (holon deploy cache-aside.holon.ts), then
//   curl http://127.0.0.1:8115/id/ada        twice: the same id, and the second run sends no request
//   curl -X PUT -d '{"id":"x"}' http://127.0.0.1:8115/id/bob      sets bob's id to "x"
import { flow, route } from "holon"
import ids from "collection:ids"
import httpbin from "host:httpbin.org"

flow("cache-aside", { port: 8115 })

// A name's id from the collection, or a fresh one fetched only when it has none, then kept.
route.get("/id/:name", async (req) => {
  const name = req.params.name
  const id = ids[name] ?? JSON.parse((await httpbin.get("/uuid")).body).uuid
  ids[name] = id
  return id
})

// Sets a name's id by hand.
route.put("/id/:name", (req) => {
  ids[req.params.name] = req.json.id
  return req.json.id
})
