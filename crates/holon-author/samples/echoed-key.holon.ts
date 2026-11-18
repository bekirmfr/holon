// A key echoed back isn't passed on: a secret goes only where its approval shows it going.
//
// The flow sends its key to httpbin.dev in a header, and httpbin.dev answers with the headers it
// got, the key among them. The flow then posts that answer to postman-echo.com. Its approval says
// "read the secret demo_key, and send it to https://httpbin.dev; what is answered to it goes to
// its callers and https://postman-echo.com": the answer may go there, the key may not (the route
// answers with what postman-echo said, so its callers are named too). The graph can't tell the
// key inside the answer, but the host finds it by its value in the second request and doesn't
// send it. That call answers {status: 0, error: "blocked_secret"}, naming the secret, never its
// value; `holon verify` on an export checks every request that carried it went where it was shown.
//
// And what the flow reads out of that echo is treated as the key too. /seen answers with the
// reply's length, its status and the method it names ("GET"): a number or a text shorter than six
// characters can't hold a piece of the key, and passes. /url answers with the address the reply
// names, a longer text: refused (500), and the run's log says what to do. The server decides by
// where a value came from and what kind it is, never by looking for the key's characters in it.
//
// Before it works, set its secret in the flow's Settings:
//   demo_key   any made-up value (6 characters or more); it is sent to httpbin.dev
//
// Try it: deploy it, set the key, then
//   curl http://127.0.0.1:8164/echo
// answers {"echoed": 200, "passed": "blocked_secret", "message": "the secret \"demo_key\" isn't sent
// to postman-echo.com: …"}. Then
//   curl http://127.0.0.1:8164/seen
// answers {"status": 200, "length": …, "method": "GET"}, and
//   curl -i http://127.0.0.1:8164/url
// answers 500: the flow may not answer its callers with what that reply said.
import { flow, route, text_length } from "holon"
import demo_key from "secret:demo_key"
import httpbin from "host:httpbin.dev"
import postman_echo from "host:postman-echo.com"

flow("echoed-key", { port: 8164 })

// Sends the key to httpbin.dev, which echoes the headers, then posts that echo to postman-echo.
route.get("/echo", async () => {
  const echoed = await httpbin.get("/anything", { headers: { "x-demo-key": demo_key } })
  const passed = await postman_echo.post("/post", {
    headers: { "content-type": "application/json" },
    body: echoed.body,
  })
  return {
    echoed: echoed.status,
    passed: passed.error ?? passed.status,
    message: passed.message ?? "",
  }
})

// A number and a short text read out of the echo are answered as usual.
route.get("/seen", async () => {
  const echoed = await httpbin.get("/anything", { headers: { "x-demo-key": demo_key } })
  const said = JSON.parse(echoed.body)
  return { status: echoed.status, length: text_length(echoed.body), method: said.method }
})

// A longer text read out of it is the key's until the flow may answer with its key: refused.
route.get("/url", async () => {
  const echoed = await httpbin.get("/anything", { headers: { "x-demo-key": demo_key } })
  const said = JSON.parse(echoed.body)
  return { url: said.url }
})
