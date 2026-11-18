// Guards: whom a tab acts as, and what becomes of an open console when its sign-in ends (R44).
// Each tab acts with the session, or the provider's ID token, it loaded with or signed in with
// since: another tab's sign-in leaves it alone. A 401 to what it is signed in with, or its ID
// token expiring (the next request, not sent without it), ends the sign-in without unmounting the
// editor: a sign-in covers it,
// and the same person signing in again goes on with the editor as it was, someone else gets one
// of their own. A pop-up back from the provider hands its token to the tab that opened it. Signing
// out asks first while open flows have changes not saved. Needed because a 401 dropped the editor
// and every unsaved flow with it, an expired ID token left the console answering nothing, and a
// second tab signing in as someone else changed whom the first acted as. If it fails, the row
// names which of these came back.

import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { held } from "./held.fixture";

type Server = typeof import("./server");
type SignIn = typeof import("./signin");

const SESSION_KEY = "holon.session";
const SIGNED_IN_KEY = "holon.signin";
const session = (name: string) => ({ token: `token-of-${name}`, name, kind: "person", role: "editor" });
/** An ID token whose claims are `claims`, unsigned (the editor reads, the server checks). */
const jwt = (claims: Record<string, unknown>) => `e30.${btoa(JSON.stringify(claims)).replace(/=+$/, "")}.sig`;
const now = () => Math.floor(Date.now() / 1000);
const CONFIG = { issuer: "https://id.example.com", client_id: "holon", authorization_endpoint: "https://id.example.com/authorize", token_endpoint: "https://id.example.com/token" };
/** Enough of the browser's crypto for a sign-in to start (its verifier, its challenge). */
const CRYPTO = { getRandomValues: (a: Uint8Array) => a.fill(7), subtle: { digest: async () => new Uint8Array(32).buffer } };

/** server.ts and signin.ts as a page loads them, with the browser's storage as it is then. */
async function load(): Promise<{ server: Server; signin: SignIn }> {
  vi.resetModules();
  return { server: await import("./server"), signin: await import("./signin") };
}

/** What `fetch` was sent with: each request's bearer. */
function answering(status: number) {
  const sent: (string | null)[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_url: string, init?: RequestInit) => {
      sent.push((init?.headers as Record<string, string> | undefined)?.Authorization ?? null);
      return new Response(JSON.stringify(status === 200 ? { name: "ada", kind: "person", role: "editor" } : { error: "signed out" }), { status });
    }),
  );
  return sent;
}

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("whom a tab acts as", () => {
  it("is whom it loaded as, whoever another tab signs in as since", async () => {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("ada")));
    const { server } = await load();
    // Another tab signs in as cy: the browser keeps cy's session, for the tabs to come.
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("cy")));
    const sent = answering(200);
    await server.whoAmI();
    expect(sent).toEqual(["Bearer token-of-ada"]);
    expect(server.getSession()?.name).toBe("ada");
    // A tab opened now takes the last one kept.
    expect((await load()).server.getSession()?.name).toBe("cy");
  });

  it("through the provider, is the ID token it loaded with, whoever another tab signs in as since", async () => {
    const ada = { idToken: jwt({ email: "ada@example.com", exp: now() + 600 }), exp: now() + 600, who: "ada@example.com" };
    localStorage.setItem(SIGNED_IN_KEY, JSON.stringify(ada));
    const { server } = await load();
    localStorage.setItem(SIGNED_IN_KEY, JSON.stringify({ ...ada, idToken: "cy's", who: "cy@example.com" }));
    const sent = answering(200);
    await server.whoAmI();
    expect(sent).toEqual([`Bearer ${ada.idToken}`]);
  });

  it("signing out forgets the browser's session only when it is this tab's", async () => {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("ada")));
    const { server } = await load();
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("cy")));
    server.setSession(null);
    expect(JSON.parse(localStorage.getItem(SESSION_KEY)!).name, "cy's, another tab's, stays").toBe("cy");
    server.setSession(session("bo") as Parameters<Server["setSession"]>[0]);
    server.setSession(null);
    expect(localStorage.getItem(SESSION_KEY)).toBeNull();
  });
});

describe("a sign-in that ends", () => {
  it("ends with a 401 to what the tab is signed in with: forgotten, and said as ended, not signed out", async () => {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("ada")));
    const { server } = await load();
    const said: string[] = [];
    const listen = (e: Event) => said.push(e.type);
    window.addEventListener("holon:session-ended", listen);
    window.addEventListener("holon:session", listen);
    answering(401);
    await server.whoAmI().catch(() => undefined);
    window.removeEventListener("holon:session-ended", listen);
    window.removeEventListener("holon:session", listen);
    expect(said).toEqual(["holon:session-ended"]);
    expect(server.getSession()).toBeNull();
    expect(localStorage.getItem(SESSION_KEY)).toBeNull();
  });

  it("doesn't end with a 401 to a token since replaced (a request sent before signing in again)", async () => {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("ada")));
    const { server } = await load();
    let answer!: (r: Response) => void;
    vi.stubGlobal(
      "fetch",
      vi.fn((_url: string, init?: RequestInit) => {
        const h = held(init);
        answer = h.answer;
        return h.answered;
      }),
    );
    const before = server.whoAmI().catch(() => undefined);
    server.setSession({ ...session("ada"), token: "token-again" } as Parameters<Server["setSession"]>[0]);
    answer(new Response("{}", { status: 401 }));
    await before;
    expect(server.getSession()?.token).toBe("token-again");
  });

  it("through the provider, ends once its ID token expires: the request isn't sent without it, the server's 401 is answered here, and the sign-in ends", async () => {
    const soon = { idToken: jwt({ email: "ada@example.com" }), exp: now() + 20, who: "ada@example.com" };
    localStorage.setItem(SIGNED_IN_KEY, JSON.stringify(soon));
    const { server, signin } = await load();
    expect(signin.signInExpired(), "within half a minute of expiring, it is expired").toBe(true);
    const ended = vi.fn();
    window.addEventListener("holon:session-ended", ended);
    const sent = answering(401);
    const said = await server.whoAmI().then(
      () => "answered",
      (e: unknown) => String(e),
    );
    window.removeEventListener("holon:session-ended", ended);
    expect(sent).toEqual([]);
    expect(said).toContain(server.SIGN_IN_FIRST);
    expect(ended).toHaveBeenCalledTimes(1);
    expect(signin.signInExpired(), "said once").toBe(false);
  });
});

describe("signing in again through the provider, in a pop-up", () => {
  it("the pop-up keeps the token, tells the tab that opened it, and closes", async () => {
    const { signin } = await load();
    const opened = vi.fn(() => ({}) as Window);
    vi.stubGlobal("open", opened);
    vi.stubGlobal("crypto", CRYPTO);
    expect(await signin.startSignIn(CONFIG, { popup: true })).toBe(true);
    const url = new URL((opened.mock.calls[0] as unknown as [string])[0]);
    const state = url.searchParams.get("state")!;
    const nonce = url.searchParams.get("nonce")!;
    // The pop-up's page, back from the provider: its own tab, which finds the sign-in by its state.
    history.replaceState(null, "", `/?code=abc&state=${state}`);
    const idToken = jwt({ email: "ada@example.com", nonce, exp: now() + 600 });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ id_token: idToken }), { status: 200 })));
    const opener = { postMessage: vi.fn() };
    vi.stubGlobal("opener", opener);
    const close = vi.fn();
    vi.stubGlobal("close", close);
    const back = await signin.finishSignIn();
    expect(back).toMatchObject({ idToken, popup: true });
    expect(opener.postMessage).toHaveBeenCalledWith(signin.SIGNED_IN_MESSAGE, location.origin);
    expect(close).toHaveBeenCalled();
    expect(JSON.parse(localStorage.getItem(SIGNED_IN_KEY)!).idToken).toBe(idToken);
  });

  it("a blocked pop-up is said, and its sign-in forgotten", async () => {
    const { signin } = await load();
    vi.stubGlobal("open", vi.fn(() => null));
    vi.stubGlobal("crypto", CRYPTO);
    expect(await signin.startSignIn(CONFIG, { popup: true })).toBe(false);
    expect(Object.keys(localStorage).filter((k) => k.startsWith("holon.signin.pending."))).toEqual([]);
  });
});

// ── The console around an ended sign-in (AuthGate) ─────────────────────────────────────────────

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

describe("the console, its sign-in ended", () => {
  let container: HTMLDivElement;
  let root: Root;
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    history.replaceState(null, "", "/");
  });

  /** The editor: what isn't saved in it, kept in its own state. */
  function Editor({ name }: { name: string }) {
    const [draft, setDraft] = useState("");
    return (
      <div>
        <span>editor of {name}</span>
        <input aria-label="draft" value={draft} onChange={(e) => setDraft(e.target.value)} />
        <button onClick={() => setDraft("unsaved text")}>type</button>
      </div>
    );
  }

  async function open() {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session("ada")));
    const { server } = await load();
    const { AuthGate } = await import("./AuthGate");
    const { DialogProvider } = await import("./Dialog");
    let me = "ada";
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const path = new URL(url, location.origin).pathname;
        if (path === "/auth/state") return new Response(JSON.stringify({ setup: false, oidc: CONFIG }), { status: 200 });
        if (path === "/auth/sign-in") {
          me = (JSON.parse(String(init?.body)) as { name: string }).name;
          return new Response(JSON.stringify({ ...session(me), token: `again-${me}` }), { status: 200 });
        }
        if (path === "/auth/me") return new Response(JSON.stringify({ name: me, kind: "person", role: "editor" }), { status: 200 });
        return new Response("{}", { status: 200 });
      }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => root.render(<DialogProvider armMs={0}>{<AuthGate>{(m) => <Editor name={m.name} />}</AuthGate>}</DialogProvider>));
    for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
    expect(container.textContent).toContain("editor of ada");
    act(() => [...container.querySelectorAll("button")].find((b) => b.textContent === "type")!.click());
    return server;
  }
  const covered = () => container.querySelector('[role="dialog"][aria-label="sign in again"]');
  const draft = () => container.querySelector<HTMLInputElement>('input[aria-label="draft"]')?.value;
  const field = (label: string) => [...container.querySelectorAll("label")].find((l) => l.textContent?.startsWith(label))!.querySelector("input")!;
  async function signInAs(name: string) {
    const set = (el: HTMLInputElement, v: string) => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    };
    act(() => set(field("name"), name));
    act(() => set(field("password"), "a long password"));
    await act(async () => covered()!.querySelector("form")!.requestSubmit());
    for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
  }
  async function end() {
    await act(async () => void window.dispatchEvent(new Event("holon:session-ended")));
  }

  it("keeps the editor, and what isn't saved in it, under a sign-in", async () => {
    await open();
    await end();
    expect(covered(), "a sign-in covers it").not.toBeNull();
    expect(draft(), "the editor is kept as it was").toBe("unsaved text");
    expect(field("name").value, "asks for whoever was signed in").toBe("ada");
  });

  it("signed in again as the same person, goes on as it was", async () => {
    const server = await open();
    await end();
    await signInAs("ada");
    expect(covered()).toBeNull();
    expect(draft()).toBe("unsaved text");
    expect(server.getSession()?.token).toBe("again-ada");
  });

  it("signed in as someone else, is drawn anew for them", async () => {
    await open();
    await end();
    await signInAs("cy");
    expect(covered()).toBeNull();
    expect(container.textContent).toContain("editor of cy");
    expect(draft(), "ada's draft isn't cy's").toBe("");
  });

  /** The provider's ID token a pop-up kept, in the browser's storage; and what says so. */
  const KEPT = () => JSON.stringify({ idToken: jwt({ email: "ada@example.com" }), exp: now() + 600, who: "ada@example.com" });
  const told = (origin: string) => new MessageEvent("message", { data: "holon:signed-in", origin });
  const seen = (kept: string) => new StorageEvent("storage", { key: SIGNED_IN_KEY, newValue: kept });
  async function say(e: Event) {
    await act(async () => void window.dispatchEvent(e));
    for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
  }
  async function openPopup() {
    vi.stubGlobal("open", vi.fn(() => ({}) as Window));
    vi.stubGlobal("crypto", CRYPTO);
    const button = [...covered()!.querySelectorAll("button")].find((b) => b.textContent === "sign in through id.example.com")!;
    await act(async () => button.click());
    for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
  }

  it("takes the token a pop-up it opened kept, told from this page's own origin", async () => {
    await open();
    await end();
    const kept = KEPT();
    localStorage.setItem(SIGNED_IN_KEY, kept);
    await say(told(location.origin));
    await say(seen(kept));
    expect(covered(), "before it opened one, another tab's sign-in is not its").not.toBeNull();
    await openPopup();
    await say(told("https://elsewhere.example"));
    expect(covered(), "another origin's message is nobody's").not.toBeNull();
    await say(told(location.origin));
    expect(covered()).toBeNull();
    expect(draft()).toBe("unsaved text");
  });

  it("takes it seen in the browser's storage, when the provider's page cut the pop-up from it", async () => {
    await open();
    await end();
    await openPopup();
    const kept = KEPT();
    localStorage.setItem(SIGNED_IN_KEY, kept);
    await say(seen(kept));
    expect(covered()).toBeNull();
    expect(draft()).toBe("unsaved text");
  });
});

describe("signing out here", () => {
  let container: HTMLDivElement;
  let root: Root;
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("asks first while open flows have changes not saved, and not otherwise", async () => {
    vi.resetModules();
    const tabs = await import("./useTabs");
    const server = await import("./server");
    const signOut = vi.spyOn(server, "signOutHere").mockResolvedValue();
    const unsaved = vi.spyOn(tabs, "unsavedFlows");
    const { useSignOut } = await import("./AuthGate");
    const { DialogProvider } = await import("./Dialog");
    let leave!: () => Promise<void>;
    function Out() {
      leave = useSignOut();
      return null;
    }
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => root.render(<DialogProvider armMs={0}><Out /></DialogProvider>));

    unsaved.mockReturnValue(2);
    let done: Promise<void>;
    await act(async () => void (done = leave()));
    const asked = document.querySelector('[aria-modal="true"]');
    expect(asked?.textContent).toContain("Sign out? 2 open flows have changes not saved: signing out closes them, and they are lost.");
    await act(async () => [...asked!.querySelectorAll("button")].find((b) => b.textContent?.endsWith("Cancel"))!.click());
    await act(async () => done);
    expect(signOut, "kept, on Cancel").not.toHaveBeenCalled();

    unsaved.mockReturnValue(0);
    await act(async () => leave());
    expect(document.querySelector('[aria-modal="true"]')).toBeNull();
    expect(signOut).toHaveBeenCalledTimes(1);
  });
});
