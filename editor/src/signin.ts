// Signing in through the server's identity provider (holon-server's oidc.rs), when it has one:
// the authorization code flow with PKCE, the editor a public client (no secret). "sign in" goes
// to the provider; it comes back here with a code, which is traded for an ID token, kept in this
// browser and sent as the bearer (server.ts) until it expires. The server checks the token and
// who it names; this side only reads when it expires, and that the nonce is the one it sent.

export type AuthConfig = { issuer: string; client_id: string; authorization_endpoint: string; token_endpoint: string };
type Pending = { verifier: string; state: string; nonce: string; config: AuthConfig; redirect: string };
export type SignedIn = { idToken: string; exp: number; who: string };

const PENDING_KEY = "holon.signin.pending";
const SIGNED_IN_KEY = "holon.signin";

const b64url = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const random = (n = 32) => b64url(crypto.getRandomValues(new Uint8Array(n)));

/** A JWT's claims, read without checking its signature (the server does that). */
export function claimsOf(jwt: string): Record<string, unknown> {
  const part = jwt.split(".")[1] ?? "";
  const json = atob(part.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(part.length / 4) * 4, "="));
  return JSON.parse(new TextDecoder().decode(Uint8Array.from(json, (c) => c.charCodeAt(0)))) as Record<string, unknown>;
}

/** Where the provider sends the browser back: this page, without its query. */
const redirectUri = () => `${location.origin}${location.pathname}`;

/** Go to the provider to sign in. */
export async function startSignIn(config: AuthConfig): Promise<void> {
  const verifier = random();
  const challenge = b64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))));
  const pending: Pending = { verifier, state: random(16), nonce: random(16), config, redirect: redirectUri() };
  sessionStorage.setItem(PENDING_KEY, JSON.stringify(pending));
  const url = new URL(config.authorization_endpoint);
  for (const [k, v] of Object.entries({
    response_type: "code",
    client_id: config.client_id,
    redirect_uri: pending.redirect,
    scope: "openid email profile",
    state: pending.state,
    nonce: pending.nonce,
    code_challenge: challenge,
    code_challenge_method: "S256",
  })) {
    url.searchParams.set(k, v);
  }
  location.assign(url.toString());
}

/** Back from the provider: the code traded for an ID token and kept. Null when this page isn't
 *  a return from signing in; why not, when it is one that failed. */
export async function finishSignIn(): Promise<SignedIn | string | null> {
  const params = new URLSearchParams(location.search);
  const code = params.get("code");
  const state = params.get("state");
  const error = params.get("error");
  if (!state || (!code && !error)) return null;
  const raw = sessionStorage.getItem(PENDING_KEY);
  sessionStorage.removeItem(PENDING_KEY);
  history.replaceState(null, "", location.pathname);
  if (error) return `sign-in: ${params.get("error_description") ?? error}`;
  const pending = raw ? (JSON.parse(raw) as Pending) : null;
  if (!pending || pending.state !== state) return "sign-in: this return doesn't match a sign-in started here";
  const res = await fetch(pending.config.token_endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "authorization_code", code: code!, redirect_uri: pending.redirect, client_id: pending.config.client_id, code_verifier: pending.verifier }),
  });
  if (!res.ok) return `sign-in: the provider refused the code (${res.status})`;
  const { id_token: idToken } = (await res.json()) as { id_token?: string };
  if (!idToken) return "sign-in: the provider sent no ID token";
  const claims = claimsOf(idToken);
  if (claims.nonce !== pending.nonce) return "sign-in: the ID token isn't for this sign-in";
  const signedIn: SignedIn = { idToken, exp: Number(claims.exp), who: String(claims.email ?? claims.name ?? claims.sub) };
  try {
    localStorage.setItem(SIGNED_IN_KEY, JSON.stringify(signedIn));
  } catch {
    // storage unavailable: signed in for this page only
    memory = signedIn;
  }
  return signedIn;
}

let memory: SignedIn | null = null;

/** Who is signed in, while their ID token hasn't expired (with half a minute to spare). */
export function signedIn(): SignedIn | null {
  let s = memory;
  try {
    s = JSON.parse(localStorage.getItem(SIGNED_IN_KEY) ?? "null") as SignedIn | null;
  } catch {
    // keep the in-memory one
  }
  return s && s.exp * 1000 > Date.now() + 30_000 ? s : null;
}

/** Whether someone signed in here, and it has since expired. */
export function signInExpired(): boolean {
  try {
    const s = JSON.parse(localStorage.getItem(SIGNED_IN_KEY) ?? "null") as SignedIn | null;
    return !!s && s.exp * 1000 <= Date.now() + 30_000;
  } catch {
    return false;
  }
}

export function signOut(): void {
  memory = null;
  try {
    localStorage.removeItem(SIGNED_IN_KEY);
  } catch {
    // nothing kept
  }
}
