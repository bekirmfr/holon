// One key for every flow: a secret the server binds, and where an admin says it may go.
//
// The flow reads `secret ops_api_key` as any flow reads a secret, and sends it to an API
// (httpbin.dev stands in: it says whether a bearer token came). The value isn't the deployment's:
// an admin binds it once, for every flow that reads a secret of that name, and says where it may
// go. This flow sends it to httpbin.dev, so it deploys and reads the server's. A flow that sent it
// anywhere else would be refused at deploy ("the server's secret "ops_api_key" may go only to
// https://httpbin.dev, and this flow sends it to ..."), unless it had its own ops_api_key under
// its Settings; and a run's request with it to another host isn't sent.
//
// Before it works, an admin binds the key (or under admin, resources, secrets):
//   printf 'ops-demo-key-123' | holon secret bind ops_api_key --to httpbin.dev
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8167/whoami
// answers {"status": 200, "authenticated": true}.
import { flow, route } from "holon"
import ops_api_key from "secret:ops_api_key"
import httpbin from "host:httpbin.dev"

flow("shared-key", { port: 8167 })

// Calls the API with the server's key.
route.get("/whoami", async () => {
  const called = await httpbin.get("/bearer", {
    headers: { authorization: `Bearer ${ops_api_key}` },
  })
  return { status: called.status, authenticated: JSON.parse(called.body).authenticated ?? false }
})
