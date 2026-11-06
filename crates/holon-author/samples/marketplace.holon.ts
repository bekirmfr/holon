// Marketplace: built with Holon.
//
// A marketplace for holons, itself a Holon flow. Publishing a listing (a holon's name, description,
// ports, manifest and its units, by hash) takes the publish key; browsing and reading one are open
// to any web page. A bad version is yanked, never edited. The editor publishes saved flows to it
// and places what it lists, checking every unit against its hash.
//
// Before it works, set its secret under the deployment's settings (the gear):
//   publish_key   the key publishers send (Authorization: Bearer <key>) to publish or yank
//
// Try it: deploy it, set it as the editor's marketplace (flows page), publish one of your saved
// flows that has no routes from the flows page, and place it from the palette's marketplace. Then
//   curl http://127.0.0.1:8120/listings      lists them
import { flow, route, secure_equals, status } from "holon"
import publish_key from "secret:publish_key"
import listings from "collection:listings"
import { now } from "holon:clock"

flow("marketplace", { port: 8120, browsers: "*" })

// Lists the listings, up to 200 at a time, from after a given id.
route.get("/listings", (req) => listings.list({ after: req.query.after ?? "", limit: 200 }))

// One listing, by its id.
route.get("/listings/:id", (req) => listings[req.params.id])

// Publishes a listing under its name and root, with the publish key (401 without it).
route.post("/listings", (req) => {
  const id = `${req.json.name}@${req.json.root}`
  const listing = secure_equals(req.headers.authorization, `Bearer ${publish_key}`)
    ? { ...req.json, published_at: now, id }
    : undefined
  listings[id] = listing
  return listing ?? status(401)
})

// Marks a listing yanked, with the publish key (401 without it); the listing itself is kept.
route.post("/listings/:id/yank", (req) => {
  const id = req.params.id
  const yanked = secure_equals(req.headers.authorization, `Bearer ${publish_key}`)
    ? { ...listings[id], yanked: true }
    : undefined
  listings[id] = yanked
  return yanked ?? status(401)
})
