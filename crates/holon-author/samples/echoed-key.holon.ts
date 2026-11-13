// A key echoed back isn't passed on: a secret goes only where its approval shows it going.
//
// The flow sends its key to httpbin.dev in a header, and httpbin.dev answers with the headers it
// got, the key among them. The flow then posts that answer to postman-echo.com. Its approval says
// "read the secret demo_key, and send it to https://httpbin.dev; what is answered to it goes to
// https://postman-echo.com": the answer may go there, the key may not. The graph can't tell the
// key inside the answer, but the host finds it by its value in the second request and doesn't
// send it. That call answers {status: 0, error: "blocked_secret"}, naming the secret, never its
// value; `holon verify` on an export checks every request that carried it went where it was shown.
//
// Before it works, set its secret in the flow's Settings:
//   demo_key   any made-up value (6 characters or more); it is sent to httpbin.dev
//
// Try it: deploy it, set the key, then
//   curl http://127.0.0.1:8164/echo
// answers {"echoed": 200, "passed": "blocked_secret", "message": "the secret \"demo_key\" isn't sent
// to postman-echo.com: …"}.
import { flow, route } from "holon"
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
