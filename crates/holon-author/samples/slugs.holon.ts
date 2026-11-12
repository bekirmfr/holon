// A flow inside a flow: holons.
//
// Two holons, each a whole flow placed as one node. slugify (lowercase, split on spaces, join with
// -) only computes, and is placed twice. publish reads and writes outside (the clock, a
// collection, state, an optional secret), and its manifest, permissions, replay and flow state
// count all of it. A saved flow without routes is placed the same way, from the palette's flows.
//
// Its optional secret, set in the flow's Settings:
//   receipt_key   signs each publish receipt; without it receipts read "unsigned"
//
// Try it: deploy it, then
//   curl -X POST -H "Content-Type: application/json" \
//     -d '{"title": "Hello Holon World", "author": "Ada Lovelace"}' http://127.0.0.1:8110/slugs
// answers the two slugs and a receipt; curl http://127.0.0.1:8110/slugs lists what was published.
// View a run to see the holons replayed and the post written, and expand a holon to see inside.
import { flow, hmac_sha256, route, status } from "holon"
import state from "state"
import receipt_key from "secret:receipt_key" with { optional: "true" }
import posts from "collection:posts"
import { now } from "holon:clock"

flow("slugs", { port: 8110 })

// Turns a text into a slug: lowercase, its words joined with -.
function slugify(text) {
  const slug = text.toLowerCase().split(" ").join("-")
  return slug
}

// Notes the slug as the last post, puts the post, and signs a receipt if it can.
function publish(slug, author) {
  state.last_post = slug
  const put = posts.put(slug, { author, at: now })
  const receipt = hmac_sha256(receipt_key, slug) ?? "unsigned"
  return { put, receipt }
}

// Slugs the title and the author, publishes, and answers both slugs with the receipt.
route.post("/slugs", (req) => {
  const title = slugify(req.json.title)
  const author = slugify(req.json.author)
  const published = publish(title ?? "", author)
  return { title, author, receipt: published.receipt } ?? status(400)
})

// Lists what was published.
route.get("/slugs", () => posts.list())
