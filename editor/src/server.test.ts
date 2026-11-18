// Every request to the server goes past the browser's HTTP cache (server.ts's `ask`): its answers
// are live and the signed-in person's own, and through the cache Chrome lets one request for an
// address go at a time, the next waiting on it. On the e2e's slow runner a read of `/auth/me` held
// there never came back, and a page stopped following the access version (can.tsx). So the
// editor's source asks the server through that one `fetch`, and what it asks goes with `no-store`.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { acceptInvite, getAuthConfig, getAuthState, getPrefs, getVersion, inviteFor, listDeployments, mayI, setSession, setUp, SIGN_IN_FIRST, signInWith, whoAmI } from "./server";

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

// With no one signed in, the server answers only what it answers anyone (can.rs's `open` routes:
// its state, its provider's settings, setting it up, signing in, an invite) and refuses the rest
// "sign in first", 401. So the editor sends nothing else: the pages that read every few seconds
// went on asking, refused each time, while the editor waited under "sign in again" (R89). Every
// request with a sign-in goes through `api`, which sends none without one; what goes as no one is
// held to the server's open routes here, so a new call can't send as no one where it is refused.
describe("with no one signed in", () => {
  const rust = (file: string) => readFileSync(join(process.cwd(), "..", "crates", "holon", "src", file), "utf8");

  it("the editor's source asks as no one only what the server answers anyone, and the rest through `api`", () => {
    const server = SOURCES.find(([path]) => path === "./server.ts")![1];
    // `ask` with no options is a GET; `postOpen` a POST.
    const asNoOne = [...server.matchAll(/\b(ask|postOpen)\(`\$\{BASE\}([^`?$]+)[^`]*`(\)|,)/g)].map(([, fn, path, then]) =>
      fn === "postOpen" ? `POST ${path}` : then === ")" ? `GET ${path}` : `${path}: an ask with options, said here with its method`,
    );
    expect([...new Set(asNoOne)].sort()).toEqual(["GET /auth/config", "GET /auth/invite", "GET /auth/state", "POST /auth/invite", "POST /auth/setup", "POST /auth/sign-in"]);
    // Each one the server answers anyone: an `open` row of can.rs's routes.
    const routes = rust("can.rs").split("pub(crate) static ROUTES")[1].split("];")[0];
    const open = [...routes.matchAll(/\bopen\("([A-Z]+)", "([^"]+)"\)/g)].map(([, method, path]) => `${method} ${path}`);
    expect(asNoOne.filter((r) => !open.includes(r)), "asked as no one, refused by the server").toEqual([]);
    // Else `ask` is `api`'s, with the bearer, and `postOpen`'s, to those same routes.
    expect([...server.matchAll(/\bask\((?!`\$\{BASE\})/g)]).toHaveLength(2);
    expect(server).toMatch(/const postOpen = \(url: string, body: unknown\) => ask\(url, \{ method: "POST", /);
  });

  // The server answers every other route only to someone signed in: what `api` answers here is
  // what it would. The one gate that lets anyone through, without accounts, is its tests' alone:
  // the server is built with `new`, and `default` (no accounts) is derived only for the tests.
  it("is answered here as the server answers it, which no server answers otherwise", () => {
    const access = rust("access.rs");
    expect(access).toMatch(/let refuse = \|message: String\| Err\(err\(401, message\)/);
    expect(access.match(/if presented\.is_empty\(\) \{\s*return refuse\("([^"]+)"/)?.[1]).toBe(SIGN_IN_FIRST);
    expect(access).toMatch(/#\[cfg_attr\(test, derive\(Default\)\)\]\r?\npub\(crate\) struct Access \{/);
    expect(access).toMatch(/fn new\(accounts: Arc<Accounts>, [^)]*\) -> Access \{\s*Access \{ accounts: Some\(accounts\),/);
  });

  it("sends nothing but what signs in: the rest is answered here as the server answers it", async () => {
    const asks: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const path = new URL(url, "http://127.0.0.1:7420").pathname;
        const as = (init?.headers as Record<string, string> | undefined)?.Authorization ?? "as no one";
        asks.push(`${init?.method ?? "GET"} ${path} ${as}`);
        const body: Record<string, unknown> = {
          "/auth/state": { setup: false, oidc: null },
          "/auth/invite": { name: "cy", token: "t", kind: "person", role: "viewer" },
          "/auth/setup": { token: "t", name: "ada", kind: "person", role: "admin" },
          "/auth/sign-in": { token: "t", name: "cy", kind: "person", role: "viewer" },
        };
        return new Response(JSON.stringify(body[path] ?? {}), { headers: { "Content-Type": "application/json" } });
      }),
    );
    for (const [what, read] of [
      ["whoAmI", whoAmI],
      ["mayI", () => mayI(["flow.view"], { flow: "d" })],
      ["getPrefs", getPrefs],
      ["listDeployments", listDeployments],
    ] as const)
      await expect(read(), what).rejects.toMatchObject({ status: 401, message: SIGN_IN_FIRST });
    await expect(getVersion()).rejects.toThrow("401");
    expect(asks).toEqual([]);
    // What anyone may ask still goes, as no one; signing in, the rest goes as them.
    await getAuthState();
    await getAuthConfig();
    await inviteFor("hi_x");
    // Signed in by the invite: what anyone may ask still goes as no one.
    await acceptInvite("hi_x", "a long enough password");
    await setUp("CODE", "ada", "a long enough password");
    await signInWith("cy", "a long enough password");
    await whoAmI();
    expect(asks).toEqual([
      "GET /auth/state as no one",
      "GET /auth/config as no one",
      "GET /auth/invite as no one",
      "POST /auth/invite as no one",
      "POST /auth/setup as no one",
      "POST /auth/sign-in as no one",
      "GET /auth/me Bearer t",
    ]);
  });
});
