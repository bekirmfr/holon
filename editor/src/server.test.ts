// Every request to the server goes past the browser's HTTP cache (server.ts's `ask`): its answers
// are live and the signed-in person's own, and through the cache Chrome lets one request for an
// address go at a time, the next waiting on it. On the e2e's slow runner a read of `/auth/me` held
// there never came back, and a page stopped following the access version (can.tsx). So the
// editor's source asks the server through that one `fetch`, and what it asks goes with `no-store`.

import { afterEach, describe, expect, it, vi } from "vitest";
import { getAuthConfig, getAuthState, getPrefs, getVersion, inviteFor, listDeployments, mayI, setSession, signInWith, whoAmI } from "./server";

/** The editor's own source, its tests left out. */
const SOURCES = Object.entries(import.meta.glob("./**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>).filter(
  ([path]) => !/\.test\.tsx?$/.test(path),
);

/** Who else the editor fetches from, and how often its source does: not the server. */
const ELSEWHERE: Record<string, number> = {
  // The marketplace: a service of its own (its listings, a publisher's key).
  "./marketplace.ts": 3,
  // The identity provider's token endpoint: the code traded for an ID token, a POST.
  "./signin.ts": 1,
};

afterEach(() => {
  vi.unstubAllGlobals();
  setSession(null);
});

describe("asking the server", () => {
  it("the editor's source asks it through one fetch, server.ts's `ask`", () => {
    const fetches = Object.fromEntries(SOURCES.map(([path, text]) => [path, (text.match(/\bfetch\(/g) ?? []).length] as const).filter(([, n]) => n > 0));
    expect(fetches).toEqual({ ...ELSEWHERE, "./server.ts": 1 });
    const server = SOURCES.find(([path]) => path === "./server.ts")![1];
    expect(server).toMatch(/const ask = \(url: string, init: RequestInit = \{\}\): Promise<Response> => fetch\(url, \{ \.\.\.init, cache: "no-store" \}\);/);
  });

  it("what it asks, signed in or not, read or changed, goes past the browser's cache", async () => {
    const asks: { path: string; cache: RequestCache | undefined }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const path = new URL(url, "http://127.0.0.1:7420").pathname;
        asks.push({ path, cache: init?.cache });
        const body: Record<string, unknown> = {
          "/auth/state": { setup: false, oidc: null },
          "/auth/invite": { name: "cy" },
          "/auth/sign-in": { token: "t", name: "cy", kind: "person", role: "viewer" },
          "/deployments": { deployments: [] },
        };
        return path === "/version" ? new Response("1.0") : new Response(JSON.stringify(body[path] ?? {}), { headers: { "Content-Type": "application/json" } });
      }),
    );
    await getAuthState();
    await inviteFor("hi_x");
    await getAuthConfig();
    await signInWith("cy", "a long enough password");
    await whoAmI();
    await mayI(["flow.view"], { flow: "d" });
    await getPrefs();
    await getVersion();
    await listDeployments();
    expect(asks.map((a) => a.path)).toEqual(["/auth/state", "/auth/invite", "/auth/config", "/auth/sign-in", "/auth/me", "/auth/me/can", "/auth/prefs", "/version", "/deployments"]);
    for (const a of asks) expect(a.cache, a.path).toBe("no-store");
  });
});
