// What the console shows as it opens is what the server and the address said, whatever order the
// server answers in (AuthGate.tsx); and once someone is signed in, by whatever way, what its
// buttons follow is that person's (can.tsx). The page is drawn under StrictMode, which asks the
// gate to decide twice as it opens, and each button's store to read twice. The e2e's slow runner:
// someone opening an invite link was shown the sign-in, not the join form (two decisions made at
// once, the first took the invite out of the address, and the second, read without it, answered
// last). Each case here opens the console with its requests held, does what the person does as
// soon as the page lets them (fills in a form, signs in again), and answers the requests in every
// order they may come back in: an invite, joining by it, a sign-in, a return from the identity
// provider (its code, taken out of the address the same way), a session kept from before, another
// tab signing in as someone else, and signing in again as someone else once a sign-in ended. The
// buttons are looked at after every answer, from the first paint: disabled while the server
// hasn't said, then the answers of whoever the tab acts as (signed out, of whom the editor kept
// under "sign in again" was drawn for), never anyone else's.
//
// The sweep keeps its own time. Each order is a run of its own, replaying the choices of the run
// it came from, and a run given the same choices must hold the same requests at each answer,
// however fast or busy the machine. So nothing the page waits on is left to the machine's time:
// - the page's timers (a read that got no answer asked again, the access version read again) run
//   only where a case waits for them;
// - the one piece of work the page has the machine do off its thread, the sign-in challenge's
//   digest (signin.ts, before the pop-up opens), is answered at once.
// On the machine's time (R83), a busy machine's digest came back after a run's settling in some
// runs and within it in others: the pop-up opened an answer later, and a replayed run held fewer
// requests than the run it replayed (a TypeError, on CI). The retry of a refused read could do the
// same. A run that holds others all the same says so.

import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Me } from "./AuthGate";

/** What the server answers, and what the console must show once it has. */
type Case = {
  name: string;
  address: string;
  /** What the browser kept from before: a session, a sign-in started at the provider. */
  kept?: Record<string, unknown>;
  /** What the server, or the provider, answers beyond what every case's server does (SERVER), by
   *  path (a GET) or method and path. */
  answers?: Record<string, () => Response>;
  /** What the person does, each as soon as the page lets them. */
  steps?: Step[];
  /** The requests (as `answers` names them) whose first gets no answer: the browser says it
   *  failed, as it does when the server is away or a connection is lost. */
  lost?: string[];
  /** Answered in the order asked until this many steps were done, in every order after: how the
   *  page got there is another case's sweep. */
  sweepFrom?: number;
  shows: (page: HTMLElement) => string | null;
};
/** `when`: whether the page lets them now, and (`quiet`) whether every request it made has been
 *  answered. */
type Step = { when: (page: HTMLElement, quiet: boolean) => boolean; then: (page: HTMLElement) => void | Promise<void> };

const json = (body: unknown, status = 200) => () => new Response(JSON.stringify(body), { status });
const form = (page: HTMLElement, name: string) => page.querySelector(`form[aria-label="${name}"]`);
const CONFIG = { issuer: "https://id.example.com", client_id: "holon", authorization_endpoint: "https://id.example.com/authorize", token_endpoint: "https://id.example.com/token" };
/** An ID token whose claims are `claims`, unsigned (the editor reads, the server checks). */
const jwt = (claims: Record<string, unknown>) => `e30.${btoa(JSON.stringify(claims)).replace(/=+$/, "")}.sig`;
const HOUR = () => Math.floor(Date.now() / 1000) + 3600;
/** A sign-in started here at the provider, which the address returns from. */
const started = { "holon.signin.pending.s1": { verifier: "v", state: "s1", nonce: "n1", config: CONFIG, redirect: "http://localhost:3000/", at: Date.now() } };

// ── who the server knows ───────────────────────────────────────────────────────────────────────
const READS_ONLY = "cy may not do that: a viewer only reads";
/** cy a viewer, ada an admin: who they are, and what `can` answers them. */
const PEOPLE: Record<string, { role: string; can: Record<string, true | string> }> = {
  cy: { role: "viewer", can: { "flow.edit": READS_ONLY, "people.view": "cy may not do that: that needs an admin" } },
  ada: { role: "admin", can: { "flow.edit": true, "people.view": true } },
};
/** Whom a bearer names: a session's token (`token-of-cy`), or the provider's ID token. */
function whose(bearer: string | null): string | null {
  if (!bearer) return null;
  if (bearer.startsWith("token-of-")) return bearer.slice("token-of-".length);
  return String(JSON.parse(atob(bearer.split(".")[1])).email).split("@")[0];
}
/** The sessions the server has ended since the page opened (signed out elsewhere, say). */
let ended = new Set<string>();
const signedInAs = (name: string) => json({ token: `token-of-${name}`, name, kind: "person", role: PEOPLE[name].role });
/** What every case's server answers: whoever the bearer names, what they may do, and a
 *  sign-in, or joining by cy's invite. */
function server(key: string, bearer: string | null, body: string | undefined): (() => Response) | undefined {
  const who = bearer && !ended.has(bearer) ? whose(bearer) : null;
  if (key === "/auth/state") return json({ setup: false, oidc: CONFIG });
  if (key === "/auth/me") return who ? json({ name: who, kind: "person", role: PEOPLE[who].role, access_version: 1 }) : json({ error: "sign in" }, 401);
  if (key === "/auth/me/can") return who ? json(PEOPLE[who].can) : json({ error: "sign in" }, 401);
  if (key === "POST /auth/sign-in") return signedInAs(JSON.parse(body ?? "{}").name);
  if (key === "POST /auth/invite") return signedInAs("cy");
  return undefined;
}

/** The editor, as far as what its buttons follow: whose it is, Save (`flow.edit`), and whether
 *  People and access is theirs (`people.view`). Before the server has answered, Save is enabled
 *  and People isn't shown: cy's and ada's are each told apart from that. */
const EDITOR: Record<string, string> = {
  cy: `editor of cy; save ${READS_ONLY}; people false`,
  ada: "editor of ada; save true; people true",
};
const editorOf = (name: string) => (page: HTMLElement) => (page.textContent?.includes(EDITOR[name]) ? null : `the editor of ${name}, its buttons ${name}'s`);
const shown = (name: string) => (page: HTMLElement) => !!page.textContent?.includes(`editor of ${name};`);

/** Types `value` into the field labelled `label`, as a person does. */
function type(page: HTMLElement, label: string, value: string) {
  const field = [...page.querySelectorAll("label")].find((l) => l.firstChild?.textContent === label)?.querySelector("input");
  if (!field) throw new Error(`no field ${label}`);
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}
const submit = (f: Element | null) => (f!.querySelector("button[type=submit]") as HTMLButtonElement).click();
/** cy's session ends at the server, and the page learns it when it comes back to the window:
 *  once her buttons are hers and the page has heard back from everything it asked (how it got
 *  there, "a session kept from before" sweeps). */
const endsForCy: Step = {
  when: (page, quiet) => quiet && !!page.textContent?.includes(EDITOR.cy),
  then: () => {
    ended.add("token-of-cy");
    window.dispatchEvent(new Event("focus"));
  },
};
const signInAgain = (page: HTMLElement) => page.querySelector('[aria-label="sign in again"]');

/** Whether the pop-up a step opens has opened. */
let popup = false;
const CASES: Case[] = [
  {
    name: "an invite link, no one signed in",
    address: "/?invite=hi_cy",
    answers: { "/auth/invite": json({ name: "cy" }) },
    shows: (page) => (form(page, "join")?.textContent?.includes("You were invited as cy") ? null : "the join form for cy"),
  },
  {
    name: "joining by an invite link",
    address: "/?invite=hi_cy",
    answers: { "/auth/invite": json({ name: "cy" }) },
    steps: [
      {
        when: (page) => !!form(page, "join"),
        then: (page) => {
          type(page, "password (at least 10 characters)", "a-long-password");
          type(page, "the password again", "a-long-password");
          submit(form(page, "join"));
        },
      },
    ],
    shows: editorOf("cy"),
  },
  {
    name: "joining by an invite link, the first read of what she may do getting no answer",
    address: "/?invite=hi_cy",
    answers: { "/auth/invite": json({ name: "cy" }) },
    lost: ["/auth/me/can"],
    steps: [
      {
        when: (page) => !!form(page, "join"),
        then: (page) => {
          type(page, "password (at least 10 characters)", "a-long-password");
          type(page, "the password again", "a-long-password");
          submit(form(page, "join"));
        },
      },
    ],
    shows: editorOf("cy"),
  },
  {
    name: "an invite link that is no one's",
    address: "/?invite=hi_gone",
    answers: { "/auth/invite": json({ error: "this invite link isn't one" }, 404) },
    shows: (page) => (form(page, "sign in")?.textContent?.includes("this invite link isn't one") ? null : "the sign-in, saying the link isn't one"),
  },
  {
    name: "signing in with a password",
    address: "/",
    steps: [
      {
        when: (page) => !!form(page, "sign in"),
        then: (page) => {
          type(page, "name", "cy");
          type(page, "password", "a-long-password");
          submit(form(page, "sign in"));
        },
      },
    ],
    shows: editorOf("cy"),
  },
  {
    name: "back from the identity provider",
    address: "/?code=c1&state=s1",
    kept: started,
    answers: { "POST /token": json({ id_token: jwt({ nonce: "n1", exp: HOUR(), email: "cy@example.com" }) }) },
    shows: editorOf("cy"),
  },
  {
    name: "back from the identity provider, which refused the code",
    address: "/?code=c1&state=s1",
    kept: started,
    answers: { "POST /token": json({ error: "invalid_grant" }, 400) },
    shows: (page) => (form(page, "sign in")?.textContent?.includes("the provider refused the code") ? null : "the sign-in, saying the provider refused the code"),
  },
  {
    name: "a session kept from before",
    address: "/",
    kept: { "holon.session": { token: "token-of-cy", name: "cy", kind: "person", role: "viewer" } },
    shows: editorOf("cy"),
  },
  {
    name: "another tab signing in as someone else",
    address: "/",
    kept: { "holon.session": { token: "token-of-cy", name: "cy", kind: "person", role: "viewer" } },
    steps: [
      {
        when: shown("cy"),
        then: () => {
          const ada = JSON.stringify({ token: "token-of-ada", name: "ada", kind: "person", role: "admin" });
          localStorage.setItem("holon.session", ada);
          window.dispatchEvent(new StorageEvent("storage", { key: "holon.session", newValue: ada }));
          window.dispatchEvent(new Event("focus"));
        },
      },
    ],
    // This tab still acts as cy (R44): what its buttons follow is cy's.
    shows: editorOf("cy"),
  },
  {
    name: "signing in again as someone else, once the sign-in ended",
    address: "/",
    sweepFrom: 1,
    kept: { "holon.session": { token: "token-of-cy", name: "cy", kind: "person", role: "viewer" } },
    steps: [
      endsForCy,
      {
        when: (page) => !!signInAgain(page),
        then: (page) => {
          type(page, "name", "ada");
          type(page, "password", "a-long-password");
          submit(signInAgain(page)!.querySelector("form"));
        },
      },
    ],
    shows: editorOf("ada"),
  },
  {
    name: "signing in again as someone else through the provider's pop-up, once the sign-in ended",
    address: "/",
    sweepFrom: 1,
    kept: { "holon.session": { token: "token-of-cy", name: "cy", kind: "person", role: "viewer" } },
    steps: [
      endsForCy,
      {
        when: (page) => !!signInAgain(page),
        then: (page) => {
          vi.stubGlobal("open", () => {
            popup = true;
            return {} as Window;
          });
          [...signInAgain(page)!.querySelectorAll("button")].find((b) => b.textContent === "sign in through id.example.com")!.click();
        },
      },
      {
        // Back at the provider's, signed in as ada, the pop-up keeps her ID token and tells this tab.
        when: () => popup,
        then: () => {
          const token = jwt({ nonce: "n", exp: HOUR(), email: "ada@example.com" });
          localStorage.setItem("holon.signin", JSON.stringify({ idToken: token, exp: HOUR(), who: "ada@example.com" }));
          window.dispatchEvent(new MessageEvent("message", { data: "holon:signed-in", origin: location.origin }));
        },
      },
    ],
    shows: editorOf("ada"),
  },
];

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let root: Root | null = null;
let container: HTMLDivElement | null = null;
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
  history.replaceState(null, "", "/");
});

/** The machine's own setTimeout, taken before any run puts the page's timers on the sweep's time. */
const machineTimeout = globalThis.setTimeout;
/** Lets the page do all it does with no time passing: what was answered, and the timers it set
 *  for at once (none set for later). */
const settle = async () => {
  for (let i = 0; i < 5; i++)
    await act(async () => {
      vi.advanceTimersByTime(0);
      await new Promise((r) => machineTimeout(r, 0));
    });
};

/** Opens the console on `c`, doing its steps as soon as the page lets them and answering its held
 *  requests as `choices` picks (the first held after them). `replays`: what the run these choices
 *  came from held at each of their answers. Says what it answered, how many were held at each
 *  answer and which, and what it then showed. */
async function open(c: Case, choices: number[], replays: string[][]) {
  // The sweep's own time, a clock of this run's: the run before's timers are gone with its clock.
  vi.useRealTimers();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  // The machine's digest, off the page's thread, answered at once: what it digests is not checked
  // here (the provider is a stand-in), only when the pop-up opens.
  const digest = vi.spyOn(crypto.subtle, "digest").mockImplementation(async () => new ArrayBuffer(32));
  history.replaceState(null, "", c.address);
  localStorage.clear();
  ended = new Set();
  popup = false;
  for (const [key, value] of Object.entries(c.kept ?? {})) localStorage.setItem(key, JSON.stringify(value));
  vi.resetModules();
  const { AuthGate } = await import("./AuthGate");
  const { DialogProvider } = await import("./Dialog");
  const { useCan } = await import("./can");
  function Editor({ me }: { me: Me }) {
    const can = useCan();
    return <span>{`editor of ${me.name}; save ${String(can.may("save"))}; people ${String(can.holds("people"))}`}</span>;
  }
  const { RETRY_MS, ASKING, UNREACHED } = await import("./can");
  const { bearer } = await import("./server");
  // From the first paint on, at every point: what the buttons show is asking, or, a read lost,
  // trying again (disabled either way), or what the server answered whoever the tab acts as
  // then. Never enabled before an answer, never another's (cy's Save shown enabled for 15 s on
  // the e2e's slow runner).
  const notTheirs: string[] = [];
  const answered: string[] = [];
  const look = () => {
    for (const [said, drawnFor, save, people] of container!.textContent?.matchAll(/editor of (\w+); save (.*?); people (true|false)/g) ?? []) {
      // Whom the tab acts as, as the page knows it: a session the server ended is the tab's
      // until the page hears so. Once it has, the tab acts as no one, and the editor kept under
      // "sign in again" (what isn't saved in it kept) is still whose it was drawn for: its
      // buttons may stay theirs, under the sign-in no one can reach them.
      const actsAs = whose(bearer() || null);
      const who = actsAs ?? drawnFor;
      const theirs = PEOPLE[who].can;
      const saveOk = save === ASKING || save === UNREACHED || save === String(theirs["flow.edit"] ?? true);
      const peopleOk = people === "false" || theirs["people.view"] === true;
      if (!saveOk || !peopleOk) notTheirs.push(`after ${answered.join(", ") || "nothing"}, acting as ${actsAs ?? "no one"}: ${said}`);
    }
  };
  const held: { path: string; answer: () => void }[] = [];
  const lost = new Set(c.lost);
  vi.stubGlobal(
    "fetch",
    vi.fn(
      (url: string, init: RequestInit = {}) =>
        new Promise<Response>((resolve, reject) => {
          const path = new URL(url, location.origin).pathname;
          const method = init.method ?? "GET";
          const key = method === "GET" ? path : `${method} ${path}`;
          const bearer = (init.headers as Record<string, string> | undefined)?.Authorization?.replace(/^Bearer /, "") ?? null;
          const body = typeof init.body === "string" ? init.body : undefined;
          if (lost.delete(key)) return held.push({ path: `${key} (lost)`, answer: () => reject(new TypeError("Failed to fetch")) });
          held.push({ path: key, answer: () => resolve((c.answers?.[key] ?? server(key, bearer, body) ?? json({}))()) });
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
          <AuthGate>{(me) => <Editor me={me} />}</AuthGate>
        </DialogProvider>
      </StrictMode>,
    ),
  );
  // Settled, and looked at.
  const settled = async () => {
    await settle();
    look();
  };
  await settled();
  const steps = c.steps ?? [];
  let done = 0;
  const act_ = async () => {
    while (done < steps.length && steps[done].when(container!, held.length === 0)) {
      const step = steps[done++];
      await act(async () => step.then(container!));
      await settled();
    }
  };
  await act_();
  const widths: number[] = [];
  const taken: number[] = [];
  const heldAt: string[][] = [];
  let differs: string | null = null;
  run: for (let waited = false; ; waited = true) {
    while (held.length > 0) {
      const pick = choices[taken.length] ?? 0;
      const now = held.map((h) => h.path);
      const then = replays[taken.length];
      if (then && now.join(" | ") !== then.join(" | ")) {
        differs = `not the same run: at answer ${taken.length + 1}, ${now.length} held (${now.join(", ")}) where the run it replays had ${then.length} (${then.join(", ")})`;
        break run;
      }
      heldAt.push(now);
      // Before `sweepFrom`'s steps are done, one order: no other to run.
      widths.push(done < (c.sweepFrom ?? 0) ? 1 : held.length);
      taken.push(pick);
      const [request] = held.splice(pick, 1);
      answered.push(request.path);
      request.answer();
      await settled();
      await act_();
    }
    // A request got no answer: the page asks again shortly, and that is answered too.
    if (!c.lost || waited) break;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(RETRY_MS + 100);
    });
    await settled();
    await act_();
  }
  const missing =
    differs ??
    (notTheirs.length > 0
      ? `the buttons of whoever the tab acts as, from the first paint: ${notTheirs[0]}`
      : done < steps.length
        ? `step ${done + 1} of ${steps.length} never possible`
        : c.shows(container));
  act(() => root!.unmount());
  container.remove();
  digest.mockRestore();
  vi.useRealTimers();
  return { answered, widths, taken, heldAt, missing };
}

describe("the console shows what the server and the address said, and the buttons follow whoever signed in, whatever order the server answers in", () => {
  for (const c of CASES) {
    it(c.name, async () => {
      const wrong: string[] = [];
      // Every order, depth first: each run answers the first held after the choices it is given,
      // and every other request it could have answered there is a run to come, which must hold
      // what this one held up to there.
      const toRun: { choices: number[]; replays: string[][] }[] = [{ choices: [], replays: [] }];
      let runs = 0;
      while (toRun.length > 0) {
        const { choices, replays } = toRun.pop()!;
        const { answered, widths, taken, heldAt, missing } = await open(c, choices, replays);
        runs += 1;
        for (let k = choices.length; k < widths.length; k++)
          for (let other = 1; other < widths[k]; other++) toRun.push({ choices: [...taken.slice(0, k), other], replays: heldAt.slice(0, k + 1) });
        if (missing) wrong.push(`answered ${answered.join(", ")}: not ${missing}`);
      }
      expect(wrong.length === 0 ? [] : [`wrong in ${wrong.length} of ${runs} orders`, ...wrong.slice(0, 3)]).toEqual([]);
    }, 300_000);
  }
});
