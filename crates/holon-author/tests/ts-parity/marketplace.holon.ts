// Marketplace (samples/marketplace.holon) in TypeScript's syntax: the same unit.
import { flow, route, secure_equals, status } from "holon"
import { now } from "holon:clock"
import listings from "collection:listings"
import publish_key from "secret:publish_key"

flow("marketplace", { port: 8120, browsers: "*" })

route.get("/listings", (req) => listings.list({ after: req.query.after ?? "", limit: 200 }))

route.get("/listings/:id", (req) => listings[req.params.id])

// Publishes a listing for whoever holds the key.
route.post("/listings", (req) => {
  const id = `${req.json.name}@${req.json.root}`
  const listing = secure_equals(req.headers.authorization, `Bearer ${publish_key}`) ? { ...req.json, published_at: now, id } : undefined
  listings[id] = listing
  return listing ?? status(401)
})

// Marks a listing yanked, for whoever holds the key.
route.post("/listings/:id/yank", (req) => {
  const id = req.params.id
  const yanked = secure_equals(req.headers.authorization, `Bearer ${publish_key}`) ? { ...listings[id], yanked: true } : undefined
  listings[id] = yanked
  return yanked ?? status(401)
})
