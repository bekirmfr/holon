// Share link: a text's methods.
//
// An article's share link, built from the query with JavaScript's text methods: trim cuts the
// spaces around the title, toLowerCase and replaceAll make it a slug (an & reads "and"), split,
// filter and join drop the doubled spaces between its words, padStart writes the id in six
// digits, and encodeURIComponent escapes the tag for the link's query. + and a template literal
// join the parts; .length counts the title as JavaScript does, in UTF-16 code units, and
// startsWith says whether the link is secure. A query without a title or an id answers 400.
//
// Try it: deploy it, then
//   curl "http://127.0.0.1:8177/link?title=%20Rust%20%26%20WebAssembly%20%20Tips%20&id=42&tag=c%2B%2B%20%26%20wasm"
//     {"length":24,"secure":true,"slug":"rust-and-webassembly-tips","title":"Rust & WebAssembly  Tips","url":"https://example.com/a/000042/rust-and-webassembly-tips?tag=c%2B%2B%20%26%20wasm"}
//   curl "http://127.0.0.1:8177/link?id=42"    400
import { flow, route, status } from "holon"

flow("share-link", { port: 8177 })

// The link: the title's slug, the id in six digits, the tag escaped (news when none is given).
route.get("/link", (req) => {
  const title = req.query.title.trim()
  const words = title.toLowerCase().replaceAll("&", "and").split(" ")
  const slug = words.filter((word) => word !== "").join("-")
  const path = "/a/" + req.query.id.padStart(6, "0") + "/" + slug
  const url = `https://example.com${path}?tag=${encodeURIComponent(req.query.tag ?? "news")}`
  const link = { title, length: title.length, slug, url, secure: url.startsWith("https://") }
  return link ?? status(400)
})
