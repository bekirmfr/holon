// Fan-out: requests made for what they do.
//
// Passes each event it is sent on to two listeners and answers 204, reading neither answer. A
// request is an effect, as a write is: once sent it can't be taken back, so it is sent whether or
// not anything reads its answer, and its answer is never the route's (a route of requests alone,
// its return left out, answers 204 too). Every hour a clock posts a heartbeat, its only work.
//
// Try it: deploy it (holon deploy fan-out.holon.ts), then
//   curl -i -X POST -d '{"type":"paid"}' http://127.0.0.1:8169/events     204, sent to both
// and the run log shows both requests and what each listener answered.
import { every, flow, route, status } from "holon"
import httpbin from "host:httpbin.org"
import postman_echo from "host:postman-echo.com"

flow("fan-out", { port: 8169 })

// Passes the event on to the billing listener and the audit one.
route.post("/events", async (req) => {
  await httpbin.post("/anything/billing", {
    headers: { "content-type": "application/json" },
    body: req.body,
  })
  await postman_echo.post("/post", {
    headers: { "content-type": "application/json" },
    body: req.body,
  })
  return status(204)
})

// A heartbeat to the billing listener: posting is all it does.
every("1h", async () => {
  await httpbin.post("/anything/heartbeat", { body: "alive" })
})
