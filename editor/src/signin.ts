// Signing in through the server's identity provider (holon-server's oidc.rs), when it has one:
// the authorization code flow with PKCE, the editor a public client (no secret). "sign in" goes
// to the provider; it comes back here with a code, which is traded for an ID token, kept in this
// browser and sent as the bearer (server.ts) until it expires. The server checks the token and
// who it names; this side only reads when it expires, and that the nonce is the one it sent.
//
// Each tab acts with the ID token it had as it loaded, or signed in with since: another tab's
// sign-in doesn't change it (R44). Signing in again over an open console, whose unsaved flows a
// page change would lose, goes to the provider in a pop-up: back, it keeps the token and tells
// the tab that opened it, which takes it (`adoptSignIn`), and closes.

import { quoted } from "./fromUrl";

export type AuthConfig = { issuer: string; client_id: string; authorization_endpoint: string; token_endpoint: string };
type Pending = { verifier: string; state: string; nonce: string; config: AuthConfig; redirect: string; popup?: boolean; at: number };
/** `popup`, as finishSignIn answers it only: this page is the pop-up a tab opened to sign in again. */
export type SignedIn = { idToken: string; exp: number; who: string; popup?: boolean };

/** A sign-in started, by its state: kept in the browser (not this tab's sessionStorage), so the
 *  pop-up it comes back to finds it. */
const PENDING_KEY = "holon.signin.pending.";
/** Where the browser keeps the ID token signed in with last (a pop-up's, for the tab that opened it):
 *  localStorage, a bearer this page's script sends, never a cookie, for the reasons server.ts gives at SESSION_KEY. */
export const SIGNED_IN_KEY = "holon.signin";
/** What the pop-up tells the tab that opened it, once it has kept the token. */
export const SIGNED_IN_MESSAGE = "holon:signed-in";

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

/** Go to the provider to sign in: this page, or (`popup`) a pop-up, which tells this tab once it
 *  is back. False when the browser blocked the pop-up. */
export async function startSignIn(config: AuthConfig, { popup = false }: { popup?: boolean } = {}): Promise<boolean> {
  const verifier = random();
  const challenge = b64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))));
  const pending: Pending = { verifier, state: random(16), nonce: random(16), config, redirect: redirectUri(), popup, at: Date.now() };
  // A sign-in started and never come back from is forgotten after an hour.
  try {
    for (const key of Object.keys(localStorage)) {
      if (!key.startsWith(PENDING_KEY)) continue;
      const at = (JSON.parse(localStorage.getItem(key) ?? "{}") as Partial<Pending>).at ?? 0;
      if (at < Date.now() - 3_600_000) localStorage.removeItem(key);
    }
  } catch {
    // nothing kept
  }
  localStorage.setItem(PENDING_KEY + pending.state, JSON.stringify(pending));
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
  if (!popup) {
    location.assign(url.toString());
    return true;
  }
  const opened = window.open(url.toString(), "holon-sign-in", "popup,width=520,height=680");
  if (!opened) localStorage.removeItem(PENDING_KEY + pending.state);
  return opened !== null;
}

/** Back from the provider: the code traded for an ID token and kept. Null when this page isn't
 *  a return from signing in; why not, when it is one that failed. */
export async function finishSignIn(): Promise<SignedIn | string | null> {
  const params = new URLSearchParams(location.search);
  const code = params.get("code");
  const state = params.get("state");
  const error = params.get("error");
  if (!state || (!code && !error)) return null;
  const raw = localStorage.getItem(PENDING_KEY + state);
  localStorage.removeItem(PENDING_KEY + state);
  history.replaceState(null, "", location.pathname);
  const pending = raw ? (JSON.parse(raw) as Pending) : null;
  // Anyone can make a link that looks like a return: only one to a sign-in started here says
  // anything of what it carries, and what the provider said is quoted as theirs.
  if (!pending || pending.state !== state) return "sign-in: this return doesn't match a sign-in started here";
  if (error) return `sign-in: your identity provider refused it: ${quoted(params.get("error_description") ?? error)}`;
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
  tabSignedIn = signedIn;
  try {
    localStorage.setItem(SIGNED_IN_KEY, JSON.stringify(signedIn));
  } catch {
    // storage unavailable: signed in for this page only
  }
  // A pop-up: the tab that opened it takes the token (told, or from the browser's storage when the
  // provider's page cut the pop-up from it), and it closes.
  if (pending.popup) {
    (window.opener as Window | null)?.postMessage(SIGNED_IN_MESSAGE, location.origin);
    window.close();
    return { ...signedIn, popup: true };
  }
  return signedIn;
}

function storedSignIn(): SignedIn | null {
  try {
    return JSON.parse(localStorage.getItem(SIGNED_IN_KEY) ?? "null") as SignedIn | null;
  } catch {
    return null;
  }
}

/** This tab's ID token: the browser's as the page loads, then its own. */
let tabSignedIn: SignedIn | null = storedSignIn();

/** Take the ID token a pop-up this tab opened just kept (SIGNED_IN_MESSAGE). */
export function adoptSignIn(): SignedIn | null {
  const was = tabSignedIn?.idToken;
  tabSignedIn = storedSignIn();
  // Signed in anew, maybe as someone else: who listens is told, as server.ts's setSession tells
  // it (can.tsx forgets what the buttons followed). Not when nothing was taken: the console,
  // signed out, would go back to its sign-in, and the open flows with it.
  if (signedIn() && tabSignedIn?.idToken !== was) window.dispatchEvent(new Event("holon:session"));
  return signedIn();
}

/** Who is signed in, while their ID token hasn't expired (with half a minute to spare). */
export function signedIn(): SignedIn | null {
  const s = tabSignedIn;
  return s && s.exp * 1000 > Date.now() + 30_000 ? s : null;
}

/** Whether this tab signed in through the provider, and it has since expired. */
export function signInExpired(): boolean {
  const s = tabSignedIn;
  return !!s && s.exp * 1000 <= Date.now() + 30_000;
}

/** Forget this tab's ID token, and the browser's when it is the same one. */
export function signOut(): void {
  const was = tabSignedIn;
  tabSignedIn = null;
  try {
    if (was && storedSignIn()?.idToken === was.idToken) localStorage.removeItem(SIGNED_IN_KEY);
  } catch {
    // nothing kept
  }
}
