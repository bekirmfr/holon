// Fetch and reshape (samples/fetch-json.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"
import httpbin from "host:httpbin.org"

flow("fetch-json", { port: 8106 })

// Gets httpbin's sample JSON and answers only the slideshow's title.
route.get("/slideshow", async () => {
  const page = await httpbin.get("/json")
  return JSON.parse(page.body).slideshow.title
})
