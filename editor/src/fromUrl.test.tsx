// Guards: nothing from the console's address is shown as markup or as the console's own words
// (EN:E4). Anyone can make a link to the console, and what it carries is theirs: a manual page's
// name (`#docs.…`), a person's or a group's (`#people.who.…`), a hash that names nothing, and a
// sign-in return's error (`?state=…&error_description=…`). Each place that shows one shows it
// quoted, on one line and cut short (fromUrl.tsx), never as markdown or HTML, and a sign-in
// return says what it carries only when it is the return of a sign-in started here. Needed
// because `#docs.<page>` put the name into markdown, where a backtick broke out to a heading and
// a link, and a return with any state put its error on the sign-in page as the console's
// sentence. If it fails, the place it names shows the address's text another way: show it with
// FromUrl, `quoted` or `cut`.

import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ACTIONS, resetCan, type Answers } from "./can";
import { DialogProvider } from "./Dialog";
import { FROM_URL_MAX } from "./fromUrl";
import { ManualPage } from "./ManualPage";
import { PeoplePage } from "./PeoplePages";
import { labelOf, nothingAt, nothingToReopen, parse, type Route } from "./routes";
import { mayI } from "./server";

vi.mock("./server", async (original) => {
  const real = await original<typeof import("./server")>();
  return {
    ...real,
    listPrincipals: vi.fn(async () => []),
    listOwned: vi.fn(async () => []),
    listRoles: vi.fn(async () => ({ roles: [], version: 1 })),
    listGroups: vi.fn(async () => ({ groups: [], version: 1 })),
    listBindings: vi.fn(async () => []),
    listDeployments: vi.fn(async () => []),
    listAccessVersions: vi.fn(async () => []),
    listAccessChanges: vi.fn(async () => []),
    getOidcSettings: vi.fn(async () => ({ issuer: null, client_id: null })),
    whoAmI: vi.fn(async () => ({ name: "ada", kind: "person", role: "admin", access_version: 1 })),
    mayI: vi.fn(),
  };
});

/** What a link's maker would have the console show: markup, and words in its voice. */
const MARK = "PWNED";
const PAYLOADS = [
  // Out of the manual's code span, to a heading and a link.
  `\`\n\n# ${MARK} heading\n\n[sign in again ${MARK}](https://evil.example/x)\n\n\``,
  `<img src=x onerror="alert(1)">${MARK}<a href="https://evil.example">here</a>`,
  `Your account is locked ${MARK}. Call +1 555 0100 to unlock it.`,
  // A quote of its own, to end the console's early and go on in its voice.
  `a” ${MARK}, call +1 555 0100 “b`,
  `${MARK} `.repeat(40),
  `${MARK}‮desrever​\nsecond line\r\n${MARK} third`,
];
/** Any of these, shown, is the payload's. */
const FRAGMENTS = [MARK, "evil.example", "555 0100", "onerror", "desrever"];
const UNSHOWN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;

/** A piece of the address as shown: one line of what is seen, at most FROM_URL_MAX and its "…". */
function shownShort(text: string, where: string) {
  expect([...text].length, `${where}: cut short`).toBeLessThanOrEqual(FROM_URL_MAX + 1);
  expect(text, `${where}: one line, nothing unseen`).not.toMatch(UNSHOWN);
}

/** A sentence of the console's: the address's text only inside its quotes, each quote short. */
function saysOnlyQuoted(said: string, where: string): string[] {
  const quotes = [...said.matchAll(/“([^“”]*)”/g)].map((m) => m[1]);
  const outside = said.replace(/“[^“”]*”/g, "");
  for (const f of FRAGMENTS) expect(outside, `${where}: "${f}" outside a quote in ${JSON.stringify(said)}`).not.toContain(f);
  expect(outside, `${where}: its quotes are whole`).not.toMatch(/[“”]/);
  for (const q of quotes) shownShort(q, where);
  return quotes;
}

/** A page: no markup from the address, its text only in a quote of its own, that quote short. */
function pageSaysOnlyQuoted(page: Element, where: string) {
  expect(page.querySelector('a[href*="evil.example"], img, script, iframe'), `${where}: markup from the address`).toBeNull();
  const walk = document.createTreeWalker(page, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const text = n.textContent ?? "";
    if (!FRAGMENTS.some((f) => text.includes(f))) continue;
    expect(n.parentElement?.closest("[data-from-url]"), `${where}: ${JSON.stringify(text.slice(0, 60))} is shown outside a quote`).not.toBeNull();
  }
  const quotes = [...page.querySelectorAll("[data-from-url]")];
  expect(quotes.length, `${where}: says what the address named`).toBeGreaterThan(0);
  for (const q of quotes) {
    expect(q.children.length, `${where}: a quote is text alone`).toBe(0);
    shownShort(q.textContent ?? "", where);
  }
}

const each = PAYLOADS.map((p, i) => [i, p] as const);

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  resetCan();
  localStorage.clear();
  vi.mocked(mayI).mockResolvedValue(Object.fromEntries(ACTIONS.map((a) => [a, true])) as Answers);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
  history.replaceState(null, "", "/");
});

async function show(page: ReactNode) {
  await act(async () => root.render(<DialogProvider armMs={0}>{page}</DialogProvider>));
  for (let i = 0; i < 3; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
}

describe("a manual page's name, from #docs.…", () => {
  it.each(each)("payload %i: the page quotes it, and its crumb is short", async (_, payload) => {
    const route = parse(`#docs.${encodeURIComponent(payload)}`);
    expect(route).toMatchObject({ kind: "docs", page: payload });
    if (route.kind !== "docs") return;
    await show(<ManualPage at={{ page: route.page!, anchor: route.anchor }} onGo={() => {}} onTry={() => null} />);
    pageSaysOnlyQuoted(container, "the manual");
    shownShort(labelOf(route), "its crumb");
  });
});

describe("a person's name, from #people.who.…", () => {
  it.each(each)("payload %i: the page quotes it, and its crumb is short", async (_, payload) => {
    const route = parse(`#people.who.${encodeURIComponent(payload)}`);
    expect(route).toEqual({ kind: "principal", name: payload });
    await show(<PeoplePage me="ada" route={route} />);
    pageSaysOnlyQuoted(container, "a person's page");
    shownShort(labelOf(route), "its crumb");
  });

  // A group's name in the address is a slug today; its page quotes one anyway.
  it.each(each)("payload %i, as a group's: the page quotes it", async (_, payload) => {
    const route: Route = { kind: "group", name: payload };
    await show(<PeoplePage me="ada" route={route} />);
    pageSaysOnlyQuoted(container, "a group's page");
    shownShort(labelOf(route), "its crumb");
  });
});

describe("an address that names nothing", () => {
  it.each(each)("payload %i: the notice and the crumb quote it", (_, payload) => {
    for (const hash of [payload, encodeURIComponent(payload)]) {
      const route = parse(`#${hash}`);
      expect(route.kind).toBe("notfound");
      if (route.kind !== "notfound") return;
      expect(saysOnlyQuoted(nothingAt(route.hash), "the notice")).toHaveLength(1);
      expect(saysOnlyQuoted(labelOf(route), "its crumb")).toHaveLength(1);
    }
  });
});

describe("a tab's slug, from #editor.…, that opens nothing after a reload", () => {
  // The address takes a slug there only of letters, digits, `-` and `_`; the notice quotes and
  // cuts whatever it is given all the same.
  it.each(each)("payload %i: the notice quotes it", (_, payload) => {
    expect(saysOnlyQuoted(nothingToReopen(payload), "the notice")).toHaveLength(1);
    expect(saysOnlyQuoted(nothingToReopen(payload, "Hello API"), "the notice, for a sample")).toHaveLength(1);
  });
  it("a slug the address takes, as long as it likes: cut short", () => {
    const slug = `${"PWNED_".repeat(30)}x`;
    expect(parse(`#editor.${slug}`)).toMatchObject({ kind: "editor", slug });
    expect(saysOnlyQuoted(nothingToReopen(slug), "the notice")).toHaveLength(1);
  });
});

describe("a sign-in return's error, from ?state=…&error=…", () => {
  /** Enough of the browser's crypto for a sign-in to start. */
  const CRYPTO = { getRandomValues: (a: Uint8Array) => a.fill(7), subtle: { digest: async () => new Uint8Array(32).buffer } };
  const CONFIG = { issuer: "https://id.example.com", client_id: "holon", authorization_endpoint: "https://id.example.com/authorize", token_endpoint: "https://id.example.com/token" };
  const carrying = (payload: string) => [
    `error=access_denied&error_description=${encodeURIComponent(payload)}`,
    `error=${encodeURIComponent(payload)}`,
  ];

  async function signin() {
    vi.resetModules();
    return import("./signin");
  }

  it.each(each)("payload %i, to no sign-in started here: says nothing of it", async (_, payload) => {
    for (const query of carrying(payload)) {
      const { finishSignIn } = await signin();
      history.replaceState(null, "", `/?state=made-up&${query}`);
      const said = await finishSignIn();
      expect(typeof said).toBe("string");
      expect(saysOnlyQuoted(said as string, "a made-up return"), "it quotes nothing").toEqual([]);
    }
  });

  it.each(each)("payload %i, to a sign-in started here: says the provider's words, quoted", async (_, payload) => {
    for (const query of carrying(payload)) {
      const { finishSignIn, startSignIn } = await signin();
      const opened = vi.fn(() => ({}) as Window);
      vi.stubGlobal("open", opened);
      vi.stubGlobal("crypto", CRYPTO);
      expect(await startSignIn(CONFIG, { popup: true })).toBe(true);
      const state = new URL((opened.mock.calls[0] as unknown as [string])[0]).searchParams.get("state")!;
      history.replaceState(null, "", `/?state=${state}&${query}`);
      const said = await finishSignIn();
      expect(typeof said).toBe("string");
      expect(saysOnlyQuoted(said as string, "the provider's refusal"), "it quotes the provider").toHaveLength(1);
    }
  });
});
