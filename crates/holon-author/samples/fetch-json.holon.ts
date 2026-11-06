// Fetch and reshape: one field from another API.
//
// Calls another API, parses its JSON and answers with the one field it needs. The flow names the
// host it calls, so deploying it asks for permission to reach that host.
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8106/slideshow
// It fetches https://httpbin.org/json and answers the slideshow's title.
import { flow, route } from "holon"
import httpbin from "host:httpbin.org"

flow("fetch-json", { port: 8106 })

// Gets httpbin's sample JSON and answers only the slideshow's title.
route.get("/slideshow", async () => {
  const page = await httpbin.get("/json")
  return JSON.parse(page.body).slideshow.title
})
