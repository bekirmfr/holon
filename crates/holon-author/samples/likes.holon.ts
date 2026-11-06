// Likes counter: runs side by side.
//
// Each like reads a post's count from a collection, adds one and writes it back. Likes that
// arrive at once run side by side; one whose count changed under it runs again, so none is lost.
//
// Try it: deploy it, then send 50 likes, 20 at a time:
//   seq 50 | xargs -P 20 -I{} curl -s -X POST http://127.0.0.1:8108/posts/1/like
//   curl http://127.0.0.1:8108/posts/1/likes    "50 likes"
//   curl http://127.0.0.1:8108/likes            each post's count
// The run log marks the runs that ran again.
import { flow, route, status } from "holon"
import likes from "collection:likes"

flow("likes", { port: 8108 })

// Adds one to the post's likes and answers the new count.
route.post("/posts/:id/like", (req) => {
  const id = req.params.id
  const n = (likes[id] ?? 0) + 1
  likes[id] = n
  return n ?? status(400)
})

// A post's likes, as text.
route.get("/posts/:id/likes", (req) => `${likes[req.params.id] ?? 0} likes`)

// Every post's count, by post id.
route.get("/likes", () => likes.list())
