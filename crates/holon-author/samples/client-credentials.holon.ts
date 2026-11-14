// A token exchange: the approval says where the secret goes, and where the token it gets goes.
//
// The flow posts its client secret to an OAuth server (Duende's public demo IdentityServer) for
// an access token, then calls an API with that token (httpbin.dev stands in: it says whether a
// bearer token came). Its approval says both: "read the secret "client_secret", and send it to
// https://demo.duendesoftware.com; what is answered to it goes to https://httpbin.dev", and the
// canvas marks each request. The token is kept out of the log by its value, as the secret is, and
// `holon verify` on an export checks every request that carried it went where it was shown.
//
// Before it works, set the secret in the flow's Settings:
//   client_secret   secret, the demo server's published secret for its client m2m
//
// Try it: deploy it, set the secret, then
//   curl http://127.0.0.1:8166/token
// answers {"issued": 200, "api": 200, "authenticated": true}.
import { flow, route } from "holon"
import client_secret from "secret:client_secret"
import demo from "host:demo.duendesoftware.com"
import httpbin from "host:httpbin.dev"

flow("client-credentials", { port: 8166 })

// Gets a token with the client secret, then calls the API with the token.
route.get("/token", async () => {
  const got = await demo.post("/connect/token", {
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: `grant_type=client_credentials&client_id=m2m&scope=api&client_secret=${client_secret}`,
  })
  const called = await httpbin.get("/bearer", {
    headers: { authorization: `Bearer ${JSON.parse(got.body).access_token}` },
  })
  return {
    issued: got.status,
    api: called.status,
    authenticated: JSON.parse(called.body).authenticated ?? false,
  }
})
