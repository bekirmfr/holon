// A stand-in OAuth token endpoint for the end-to-end tests (the support-inbox sample's mailbox,
// logged in with OAuth, server.mjs): plain HTTP on loopback, the one place Holon sends a token
// request without TLS. It answers POST /token as Microsoft's and Google's do, the client's id and
// secret in the form: a bearer token for the one client it knows, 401 invalid_client to any other.

import { createServer } from "node:http";

/** Listen on a free loopback port, giving `token` to the client `id` with `secret` (the client
 *  credentials grant); resolves to the port. */
export function tokenEndpoint(id, secret, token) {
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const form = new URLSearchParams(body);
      const answer = (status, json) => {
        res.writeHead(status, { "Content-Type": "application/json" });
        res.end(JSON.stringify(json));
      };
      if (req.method !== "POST" || req.url.split("?")[0] !== "/token") answer(404, { error: "not_found" });
      else if (form.get("client_id") !== id || form.get("client_secret") !== secret) answer(401, { error: "invalid_client", error_description: "The client or its secret isn't known here." });
      else if (form.get("grant_type") !== "client_credentials") answer(400, { error: "unsupported_grant_type" });
      else answer(200, { access_token: token, token_type: "Bearer", expires_in: 3600 });
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}
