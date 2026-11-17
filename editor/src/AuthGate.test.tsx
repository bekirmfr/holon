// What the console shows as it opens is what the server and the address said, whatever order the
// server answers in (AuthGate.tsx). The page is drawn under StrictMode, which asks the gate to
// decide twice as it opens. The e2e's slow runner: someone opening an invite link was shown the
// sign-in, not the join form. Two decisions were made at once, the first took the invite out of
// the address, and the second, read without it, answered last. Each case here opens the console
// with its requests held, and answers them in every order they may come back in: an invite, a
// return from the identity provider (its code, taken out of the address the same way), a session
// kept from before.

import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

/** What the server answers, and what the console must show once it has. */
type Case = {
  name: string;
  address: string;
  /** What the browser kept from before: a session, a sign-in started at the provider. */
  kept?: Record<string, unknown>;
  /** What the server, or the provider, answers on a path other than /auth/state's. */
  answers: Record<string, () => Response>;
  shows: (page: HTMLElement) => string | null;
};

const json = (body: unknown, status = 200) => () => new Response(JSON.stringify(body), { status });
const form = (page: HTMLElement, name: string) => page.querySelector(`form[aria-label="${name}"]`);
const CONFIG = { issuer: "https://id.example.com", client_id: "holon", authorization_endpoint: "https://id.example.com/authorize", token_endpoint: "https://id.example.com/token" };
/** An ID token whose claims are `claims`, unsigned (the editor reads, the server checks). */
const jwt = (claims: Record<string, unknown>) => `e30.${btoa(JSON.stringify(claims)).replace(/=+$/, "")}.sig`;
/** A sign-in started here at the provider, which the address returns from. */
const started = { "holon.signin.pending.s1": { verifier: "v", state: "s1", nonce: "n1", config: CONFIG, redirect: "http://localhost:3000/", at: Date.now() } };

const CASES: Case[] = [
  {
    name: "an invite link, no one signed in",
    address: "/?invite=hi_cy",
    answers: { "/auth/invite": json({ name: "cy" }) },
    shows: (page) => (form(page, "join")?.textContent?.includes("You were invited as cy") ? null : "the join form for cy"),
  },
  {
    name: "an invite link that is no one's",
    address: "/?invite=hi_gone",
    answers: { "/auth/invite": json({ error: "this invite link isn't one" }, 404) },
    shows: (page) => (form(page, "sign in")?.textContent?.includes("this invite link isn't one") ? null : "the sign-in, saying the link isn't one"),
  },
  {
    name: "back from the identity provider",
    address: "/?code=c1&state=s1",
    kept: started,
    answers: {
      "/token": json({ id_token: jwt({ nonce: "n1", exp: Math.floor(Date.now() / 1000) + 3600, email: "cy@example.com" }) }),
      "/auth/me": json({ name: "cy", kind: "person", role: "viewer" }),
    },
    shows: (page) => (page.textContent?.includes("editor of cy") ? null : "the editor of cy"),
  },
  {
    name: "back from the identity provider, which refused the code",
    address: "/?code=c1&state=s1",
    kept: started,
    answers: { "/token": json({ error: "invalid_grant" }, 400) },
    shows: (page) => (form(page, "sign in")?.textContent?.includes("the provider refused the code") ? null : "the sign-in, saying the provider refused the code"),
  },
  {
    name: "a session kept from before",
    address: "/",
    kept: { "holon.session": { token: "token-of-ada", name: "ada", kind: "person", role: "editor" } },
    answers: { "/auth/me": json({ name: "ada", kind: "person", role: "editor" }) },
    shows: (page) => (page.textContent?.includes("editor of ada") ? null : "the editor of ada"),
  },
];

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let root: Root | null = null;
let container: HTMLDivElement | null = null;
afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  history.replaceState(null, "", "/");
});

const settle = async () => {
  for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
};

/** Opens the console on `c`, answering its held requests as `choices` picks (the first held after
 *  them). Says what it answered, how many were held at each answer, and what it then showed. */
async function open(c: Case, choices: number[]) {
  history.replaceState(null, "", c.address);
  localStorage.clear();
  for (const [key, value] of Object.entries(c.kept ?? {})) localStorage.setItem(key, JSON.stringify(value));
  vi.resetModules();
  const { AuthGate } = await import("./AuthGate");
  const { DialogProvider } = await import("./Dialog");
  const held: { path: string; answer: () => void }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(
      (url: string) =>
        new Promise<Response>((resolve) => {
          const path = new URL(url, location.origin).pathname;
          held.push({ path, answer: () => resolve(path === "/auth/state" ? json({ setup: false, oidc: CONFIG })() : (c.answers[path] ?? json({}))()) });
        }),
    ),
  );
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <StrictMode>
        <DialogProvider armMs={0}>
          <AuthGate>{(me) => <span>editor of {me.name}</span>}</AuthGate>
        </DialogProvider>
      </StrictMode>,
    ),
  );
  await settle();
  const answered: string[] = [];
  const widths: number[] = [];
  const taken: number[] = [];
  while (held.length > 0) {
    const pick = choices[taken.length] ?? 0;
    widths.push(held.length);
    taken.push(pick);
    const [request] = held.splice(pick, 1);
    answered.push(request.path);
    request.answer();
    await settle();
  }
  const missing = c.shows(container);
  act(() => root!.unmount());
  container.remove();
  return { answered, widths, taken, missing };
}

describe("the console shows what the server and the address said, whatever order the server answers in", () => {
  for (const c of CASES) {
    it(c.name, async () => {
      const wrong: string[] = [];
      // Every order, depth first: each run answers the first held after the choices it is given,
      // and every other request it could have answered there is a run to come.
      const toRun: number[][] = [[]];
      while (toRun.length > 0) {
        const choices = toRun.pop()!;
        const { answered, widths, taken, missing } = await open(c, choices);
        for (let k = choices.length; k < widths.length; k++) for (let other = 1; other < widths[k]; other++) toRun.push([...taken.slice(0, k), other]);
        if (missing) wrong.push(`answered ${answered.join(", ")}: not ${missing}`);
      }
      expect(wrong).toEqual([]);
    });
  }
});
