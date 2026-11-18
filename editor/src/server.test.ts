// Every request to the server goes past the browser's HTTP cache (server.ts's `ask`): its answers
// are live and the signed-in person's own, and through the cache Chrome lets one request for an
// address go at a time, the next waiting on it. On the e2e's slow runner a read of `/auth/me` held
// there never came back, and a page stopped following the access version (can.tsx). So the
// editor's source asks the server through that one `fetch`, and what it asks goes with `no-store`.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { held } from "./held.fixture";
import {
  acceptInvite,
  ANSWER_WITHIN,
  askingAgainNow,
  getAuthConfig,
  getAuthState,
  getPrefs,
  getVersion,
  inviteFor,
  listDeployments,
  mayI,
  pickFolder,
  READ_AGAIN_MS,
  reloadPermissions,
  replayVersion,
  setSession,
  setUp,
  SIGN_IN_FIRST,
  signInWith,
  Unanswered,
  whoAmI,
} from "./server";

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
    expect(server).toMatch(/const answered = fetch\(url, \{ \.\.\.init, cache: "no-store", signal: stop\.signal \}\);/);
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
    expect([...server.matchAll(/(?<!function )\bask\((?!`\$\{BASE\})/g)]).toHaveLength(2);
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

// On a busy machine the server's HTTP library can leave a new connection unread until another one
// closes: a request written into it gets no answer at all, neither a status nor an error, and a
// page waited on it for a minute or more, its buttons disabled (R97). Every request now has its
// time, by what it asks (ANSWER_WITHIN): a read past it is given up, its connection dropped, and
// asked again, the page told meanwhile; a read that has the server work first gets longer, and
// isn't asked again; a change is never sent twice, and given up it is said as maybe done; the
// folder picker waits on a person. Each row: what is sent, when, what is dropped, what is said.
describe("a request the server doesn't answer", () => {
  /** Each request sent: its method and path, whether it was given up (its connection dropped),
   *  and its answer, held until given (as a connection the server left unread holds it), or
   *  given late, after it was given up. Held as a browser's fetch holds it (held.fixture.ts):
   *  given up, it rejects inside the abort, before the editor's own timer has said so. */
  type Sent = { method: string; path: string; dropped: () => boolean; answer: (body: unknown) => void };
  let sent: Sent[];
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    setSession({ token: "t", name: "cy", kind: "person", role: "editor" });
    sent = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        const h = held(init);
        sent.push({
          method: init?.method ?? "GET",
          path: new URL(url, "http://127.0.0.1:7420").pathname,
          dropped: h.dropped,
          answer: (body) => h.answer(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } })),
        });
        return h.answered;
      }),
    );
  });
  afterEach(() => vi.useRealTimers());
  const wait = (ms: number) => vi.advanceTimersByTimeAsync(ms);
  const outcome = (p: Promise<unknown>) => p.then((v) => ({ answered: v }), (e: unknown) => ({ said: e }));

  it("a read with no answer is given up at its time, its connection dropped, and asked again; the page is told meanwhile", async () => {
    const me = whoAmI();
    await wait(ANSWER_WITHIN.read - 1);
    expect(sent.map((s) => s.path)).toEqual(["/auth/me"]);
    expect(askingAgainNow()).toBe(false);
    await wait(1);
    expect(sent[0].dropped(), "its connection dropped").toBe(true);
    expect(askingAgainNow(), "trying again").toBe(true);
    await wait(READ_AGAIN_MS[0]);
    expect(sent.map((s) => s.path)).toEqual(["/auth/me", "/auth/me"]);
    sent[1].answer({ name: "cy", kind: "person", role: "editor" });
    expect((await me).name).toBe("cy");
    expect(askingAgainNow()).toBe(false);
  });

  it("an answer within its time, however late in it, is taken, and nothing is asked twice", async () => {
    const me = whoAmI();
    await wait(ANSWER_WITHIN.read - 1);
    sent[0].answer({ name: "cy", kind: "person", role: "editor" });
    expect((await me).name).toBe("cy");
    await wait(3 * ANSWER_WITHIN.read);
    expect(sent).toHaveLength(1);
    expect(sent[0].dropped()).toBe(false);
  });

  it("a read given up whose answer comes anyway, late: the one asked again is taken, never the late one", async () => {
    const may = mayI(["flow.view"]);
    await wait(ANSWER_WITHIN.read + READ_AGAIN_MS[0]);
    expect(sent).toHaveLength(2);
    sent[0].answer({ "flow.view": "the late answer, read before" });
    sent[1].answer({ "flow.view": true });
    expect(await may).toEqual({ "flow.view": true });
  });

  it("no answer to any of its three tries is said, and nothing more is sent", async () => {
    const prefs = outcome(getPrefs());
    await wait(3 * ANSWER_WITHIN.read + READ_AGAIN_MS[0] + READ_AGAIN_MS[1]);
    const { said } = (await prefs) as { said: unknown };
    expect(said).toBeInstanceOf(Unanswered);
    expect((said as Error).message).toBe(`the server didn't answer, asked 3 times, ${ANSWER_WITHIN.read / 1000} s each: it may be busy, or not running`);
    expect(sent.map((s) => s.path)).toEqual(["/auth/prefs", "/auth/prefs", "/auth/prefs"]);
    expect(sent.every((s) => s.dropped())).toBe(true);
    expect(askingAgainNow()).toBe(false);
    await wait(60_000);
    expect(sent).toHaveLength(3);
  });

  it("a change with no answer is sent once, never again, and said as maybe done; the buttons read again", async () => {
    const told = vi.fn();
    window.addEventListener("holon:can", told);
    const reload = outcome(reloadPermissions());
    await wait(ANSWER_WITHIN.change);
    const { said } = (await reload) as { said: unknown };
    window.removeEventListener("holon:can", told);
    expect(said).toBeInstanceOf(Unanswered);
    expect((said as Error).message).toBe(
      `the server didn't answer in ${ANSWER_WITHIN.change / 60_000} minutes: it may or may not have been done; the page shows what it holds when it reads it again`,
    );
    expect(sent.map((s) => `${s.method} ${s.path}`)).toEqual(["POST /permissions/reload"]);
    expect(sent[0].dropped()).toBe(true);
    expect(told, "what the buttons follow is read again").toHaveBeenCalledTimes(1);
    await wait(ANSWER_WITHIN.change);
    expect(sent).toHaveLength(1);
  });

  it("a read that has the server work first is given its longer time, and isn't asked again", async () => {
    const replay = outcome(replayVersion("d", "u"));
    await wait(3 * ANSWER_WITHIN.read);
    expect(sent).toHaveLength(1);
    expect(sent[0].dropped()).toBe(false);
    await wait(ANSWER_WITHIN.work);
    expect(((await replay) as { said: unknown }).said).toBeInstanceOf(Unanswered);
    expect(sent.map((s) => s.path)).toEqual(["/deployments/d/replay"]);
  });

  // The rows above hold their answers as a browser's fetch does. In Chrome, aborted from a timer
  // and raced as `once` raced it before, the fetch's own AbortError won over the timer's
  // `Unanswered`, and the editor took a give-up for another failure: a read wasn't asked again,
  // a change wasn't said as maybe done. Node's fetch, and a stand-in that only held its answer,
  // let the timer win. So the stand-ins are held to the browser's order, here, and every one
  // holds its answer through held.fixture.ts.
  it("a stand-in's held answer, given up, rejects as a browser's fetch does: inside the abort, with its reason", async () => {
    const raced = (abort: (stop: AbortController) => void) => {
      const stop = new AbortController();
      const h = held({ signal: stop.signal });
      const timer = new Promise<never>((_, reject) => {
        abort(stop);
        reject(new Error("the timer's own, after the abort"));
      });
      return Promise.race([h.answered, timer]).then(
        () => new Error("answered"),
        (e: unknown) => e as Error,
      );
    };
    const plain = await raced((stop) => stop.abort());
    expect(plain.name, "the fetch's own, before the timer's").toBe("AbortError");
    expect(plain).not.toBeInstanceOf(Unanswered);
    expect(await raced((stop) => stop.abort(new Unanswered("given up"))), "the reason given").toBeInstanceOf(Unanswered);
    const tests = Object.entries(import.meta.glob("./**/*.test.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>);
    const own = new RegExp("new Promise" + "<Response>");
    expect(
      tests.filter(([, text]) => own.test(text)).map(([path]) => path),
      "a stand-in fetch holding its answer otherwise",
    ).toEqual([]);
  });

  it("the host's folder picker waits on a person, as long as they take", async () => {
    const picked = pickFolder("C:/");
    await wait(10 * 60_000);
    expect(sent).toHaveLength(1);
    expect(sent[0].dropped()).toBe(false);
    sent[0].answer({ path: "C:/data" });
    expect(await picked).toEqual({ path: "C:/data" });
  });
});
