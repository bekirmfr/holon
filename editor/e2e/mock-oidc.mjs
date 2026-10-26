// A stand-in identity provider for the end-to-end tests: just enough OpenID Connect for the
// editor to sign in through it and holon-server to check what it signs. On 127.0.0.1:7530 (the
// slot's, ports.mjs), it publishes its discovery document and its key (an RSA key made at startup,
// never kept), shows a page to pick who to sign in as (no passwords: it is a test fixture), and
// trades the code for an ID token once the PKCE verifier matches. Nothing here is fit for anything
// but tests.

import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { createServer } from "node:http";
import { PORTS } from "./ports.mjs";

const PORT = PORTS.oidc;
const ISSUER = `http://127.0.0.1:${PORT}`;
const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: "jwk" }), kid: "e2e", alg: "RS256", use: "sig" };
/** code → what it was issued for. */
const codes = new Map();

const b64url = (buf) => Buffer.from(buf).toString("base64url");
const idToken = (claims) => {
  const head = b64url(JSON.stringify({ alg: "RS256", typ: "JWT", kid: "e2e" }));
  const body = b64url(JSON.stringify(claims));
  return `${head}.${body}.${b64url(sign("sha256", Buffer.from(`${head}.${body}`), privateKey))}`;
};
const send = (res, status, type, body) => {
  res.writeHead(status, { "Content-Type": type, "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "Content-Type" });
  res.end(body);
};
const json = (res, status, value) => send(res, status, "application/json", JSON.stringify(value));
const escape = (s) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

createServer((req, res) => {
  const url = new URL(req.url, ISSUER);
  if (req.method === "OPTIONS") return send(res, 204, "text/plain", "");
  if (url.pathname === "/.well-known/openid-configuration") {
    return json(res, 200, { issuer: ISSUER, authorization_endpoint: `${ISSUER}/authorize`, token_endpoint: `${ISSUER}/token`, jwks_uri: `${ISSUER}/jwks` });
  }
  if (url.pathname === "/jwks") return json(res, 200, { keys: [jwk] });
  if (url.pathname === "/authorize") {
    const p = url.searchParams;
    const who = p.get("login_hint");
    // Picked: issue a code for them and go back.
    if (who) {
      const code = b64url(randomBytes(16));
      codes.set(code, { email: who, nonce: p.get("nonce"), client: p.get("client_id"), redirect: p.get("redirect_uri"), challenge: p.get("code_challenge") });
      const back = new URL(p.get("redirect_uri"));
      back.searchParams.set("code", code);
      back.searchParams.set("state", p.get("state"));
      res.writeHead(302, { Location: back.toString() });
      return res.end();
    }
    const pick = (email) => {
      const next = new URL(url);
      next.searchParams.set("login_hint", email);
      return `<p><a href="${escape(next.toString())}">sign in as ${escape(email)}</a></p>`;
    };
    return send(res, 200, "text/html", `<!doctype html><title>test sign-in</title><h1>test sign-in</h1>${["ada@example.com", "cy@example.com", "eve@example.com"].map(pick).join("")}`);
  }
  if (url.pathname === "/token" && req.method === "POST") {
    let raw = "";
    req.on("data", (d) => (raw += d));
    req.on("end", () => {
      const form = new URLSearchParams(raw);
      const issued = codes.get(form.get("code"));
      codes.delete(form.get("code"));
      const verified = issued && createHash("sha256").update(form.get("code_verifier") ?? "").digest("base64url") === issued.challenge;
      if (!verified || form.get("client_id") !== issued.client || form.get("redirect_uri") !== issued.redirect) {
        return json(res, 400, { error: "invalid_grant" });
      }
      const now = Math.floor(Date.now() / 1000);
      json(res, 200, {
        token_type: "Bearer",
        id_token: idToken({ iss: ISSUER, aud: issued.client, sub: `sub-${issued.email}`, email: issued.email, email_verified: true, nonce: issued.nonce, iat: now, exp: now + 600 }),
      });
    });
    return;
  }
  send(res, 404, "text/plain", "not found");
}).listen(PORT, "127.0.0.1");
