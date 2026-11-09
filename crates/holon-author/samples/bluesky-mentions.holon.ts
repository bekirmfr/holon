// Bluesky mentions: Bluesky's posts as they are written, from a feed that replays from a cursor.
//
// Jetstream sends each post on Bluesky as a message on a WebSocket, numbered (payload.seq), and
// sends again what came after a number when a connection asks from it (?cursor=…). Each post the
// feed sends starts one run of the handler; a post that mentions the word PUT /watch sets is kept,
// by its at:// address. The handler is lazy: a post is cheap, and where the host is in the feed
// moves once the record is synced. When the connection is lost, or the server stopped, the host
// opens it again from where it is (--resume query:cursor): the posts that came meanwhile run, and
// the one at the cursor, sent again, is passed over. A post sent twice writes the same item.
//
// Before it works, an admin binds the feed, its URL in a secret source:
//   HOLON_SECRET_POSTS_URL=wss://jetstream.us-east.bsky.network/xrpc/network.bsky.jetstream.subscribeEvents?collections=app.bsky.feed.post
//   holon feed add posts --url env:HOLON_SECRET_POSTS_URL --id payload.seq --resume query:cursor --only payload.operation=create
//
// Try it: deploy it, then
//   curl -X PUT -d '{"word":"holon"}' http://127.0.0.1:8192/watch
//   curl http://127.0.0.1:8192/mentions       {"at://did:plc:…/app.bsky.feed.post/…": {"text": …}, …}
import { flow, on, route } from "holon"
import mentions from "collection:mentions"
import watch from "collection:watch"
import posts from "feed:posts"
import { now } from "holon:clock"

flow("bluesky-mentions", { port: 8192 })

// Each post created: kept when it mentions the word, by its address.
on.feed(
  posts,
  (e) => {
    const post = e.data.payload
    const text = post.record.text
    const kept = text.toLowerCase().includes(watch["word"].text)
      ? { text, by: post.did, at: post.time, seq: e.id }
      : undefined
    mentions[`at://${post.did}/${post.collection}/${post.rkey}`] = kept
  },
  { log: "lazy" },
)

route.put("/watch", (req) => {
  const word = req.json.word.toLowerCase()
  watch["word"] = { text: word, since: now }
  return { word }
})

route.get("/mentions", () => mentions.list())
