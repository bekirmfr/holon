// A Basic header built from a secret: the log marks what was computed, and verify runs it again.
//
// A Basic authorization header is base64 of "user:password": computed from the secret, no longer
// the secret as it is. The log never holds such a value. It writes a mark in its place wherever
// the run had it (`‹derived #7c1e9a02›`: in the step that made it, in the header sent, in what
// the far side echoed back), and the run's entry lists the value's whole hash, keyed so the
// value can't be guessed from it. `holon verify` on an export then runs /call again: the one
// step that encoded the secret is taken as recorded and named, and every step after it is
// computed on the mark and compared.
//
// A mark stands for a text of six characters or more. /kind computes a true or false from the
// secret (how it begins), which no mark can stand for: the log withholds what follows from it,
// and `holon verify` says that run wasn't run again, and why. To keep such runs checked, keep
// facts like that out of the secret: a setting of its own beside it.
//
// Before it works, set the secret in the flow's Settings:
//   api_password   secret, any made-up value (6 characters or more); it is sent to httpbin.dev
//
// Try it: deploy it, set the secret, then
//   curl http://127.0.0.1:8201/call
//   curl http://127.0.0.1:8201/kind
// each answers {"status": 200}.
import { base64, flow, route } from "holon"
import api_password from "secret:api_password"
import httpbin from "host:httpbin.dev"

flow("basic-auth", { port: 8201 })

// Calls the API with a Basic header built from the secret.
route.get("/call", async () => {
  const called = await httpbin.get("/headers", {
    headers: { authorization: `Basic ${base64(`holon:${api_password}`)}` },
  })
  return { status: called.status }
})

// Tells the API whether it holds a live key, from how the secret begins.
route.get("/kind", async () => {
  const live = api_password.startsWith("live_")
  const called = await httpbin.get("/status/200", { headers: { "x-key-live": String(live) } })
  return { status: called.status }
})
