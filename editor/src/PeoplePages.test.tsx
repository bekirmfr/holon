// Guards: People and access follows `can`, as every other page does: to someone who may view people
// and change nothing, every control there that sends a change (a row's button or list, a form's
// submit) is disabled, its title the server's refusal; an agent's tokens stay theirs to make and
// revoke when they own it (can.rs's `owns`); and to an admin each row's control is there to use.
// Needed because the People rows' buttons followed nothing: a viewer bound people.view saw email,
// reset, disable, remove, a role list and every form enabled, and learned only from the refusal
// after clicking (R44). If it fails, the control it names sends what its viewer may not: gate it
// (can.tsx's BUTTONS), or, if it sends nothing, say so in DRAFTS.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ACTIONS, resetCan, type Answers } from "./can";
import { DialogProvider } from "./Dialog";
import { PeoplePage } from "./PeoplePages";
import type { Route } from "./routes";
import { mayI, type BindingEntry, type GroupEntry, type HeldBinding, type Kind, type Principal, type RoleEntry } from "./server";

const at = 1_700_000_000;
const held = (role: string): HeldBinding => ({ id: 1, role, allows: [], scope: "host", group: null, by: "ada", at, expires_at: null, expired: false });
const principal = (name: string, kind: Kind, role: string, more: Partial<Principal> = {}): Principal => ({
  name,
  kind,
  role: role as Principal["role"],
  bindings: [held(role)],
  display: null,
  email: null,
  disabled: false,
  created_at: at,
  created_by: "ada",
  owner: null,
  owner_disabled: false,
  invited: false,
  last_seen: null,
  ...more,
});
const PEOPLE: Principal[] = [
  principal("ada", "person", "admin"),
  principal("bo", "person", "viewer"),
  // bo's agent: its tokens are bo's to make and revoke.
  principal("ci", "agent", "agent", { owner: "bo", tokens: [{ id: "t1", principal: "ci", label: "laptop", created_at: at, created_by: "bo" }] }),
  principal("scout", "agent", "agent", { owner: "ada", tokens: [{ id: "t2", principal: "scout", label: "runner", created_at: at, created_by: "ada" }] }),
];
const role = (name: string, kinds: Kind[], builtin = true): RoleEntry => ({ name, about: `${name}'s`, allows: builtin ? [] : ["people.view"], kinds, builtin, bound: 1 });
const ROLES = [role("admin", ["person"]), role("viewer", ["person", "agent", "service"]), role("agent", ["agent", "service"]), role("keeper", ["person"], false)];
const GROUPS: GroupEntry[] = [{ name: "payments", about: null, members: ["bo"], bound: 0 }];
const BINDINGS: BindingEntry[] = [{ id: 1, who: { principal: "bo" }, role: "viewer", scope: "host", by: "ada", at, expired: false }];

vi.mock("./server", async (original) => {
  const real = await original<typeof import("./server")>();
  return {
    ...real,
    listPrincipals: vi.fn(async () => PEOPLE),
    listOwned: vi.fn(async () => PEOPLE.filter((p) => p.owner === "bo")),
    listRoles: vi.fn(async () => ({ roles: ROLES, version: 3 })),
    listGroups: vi.fn(async () => ({ groups: GROUPS, version: 3 })),
    listBindings: vi.fn(async () => BINDINGS),
    listDeployments: vi.fn(async () => []),
    listAccessVersions: vi.fn(async () => []),
    listAccessChanges: vi.fn(async () => []),
    getOidcSettings: vi.fn(async () => ({ issuer: "https://id.example.com", client_id: "holon" })),
    whoAmI: vi.fn(async () => ({ name: "bo", kind: "person", role: "viewer", access_version: 3 })),
    mayI: vi.fn(),
  };
});

/** What `can` answers: `holding` true, every other action refused in the server's words. */
const answering = (holding: string[]): Answers => Object.fromEntries(ACTIONS.map((a) => [a, holding.includes(a) ? true : `bo may not do that: ${a} is not among what you hold on this host`]));
const VIEWER = answering(["people.view", "role.view"]);
const ADMIN = answering([...ACTIONS]);

/** Every page of People and access: its tabs, a principal's page (bo's agent's and ada's), a group's. */
const PAGES: Route[] = [
  ...(["people", "agents", "groups", "roles", "bindings", "history", "sign-in"] as const).map((tab): Route => ({ kind: "people", tab })),
  { kind: "principal", name: "ci" },
  { kind: "principal", name: "scout" },
  { kind: "principal", name: "bo" },
  { kind: "group", name: "payments" },
];

/** Controls that send nothing, by name: they copy, close what was shown, or fill in a draft. */
const DRAFTS = ["copy", "done", "cancel"];

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
beforeEach(() => resetCan());
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

async function show(route: Route) {
  if (!container) {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  }
  await act(async () =>
    root!.render(
      <DialogProvider armMs={0}>
        <PeoplePage me="bo" route={route} />
      </DialogProvider>,
    ),
  );
  for (let i = 0; i < 3; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
}

/** A control as a person finds it: its label, or its text. */
const called = (el: Element) => el.getAttribute("aria-label") ?? el.textContent?.trim() ?? "";

/** The controls on the page that send a change: a button outside a form or a form's submit, and a
 *  list outside a form (a row's, which acts as it is changed). */
function sending(): HTMLElement[] {
  return [...container!.querySelectorAll<HTMLElement>("button, select")].filter((el) => {
    if (DRAFTS.includes(called(el))) return false;
    const form = el.closest("form");
    if (el instanceof HTMLSelectElement) return !form;
    return !form || (el as HTMLButtonElement).type === "submit";
  });
}

describe("People and access, to someone who may view people and change nothing", () => {
  it.each(PAGES.map((r) => [JSON.stringify(r), r] as const))("%s: each control that sends a change is disabled, saying why", async (_, route) => {
    vi.mocked(mayI).mockResolvedValue(VIEWER);
    await show(route);
    const controls = sending();
    // History only lists what changed (and links to what waits, in Approvals).
    if (!(route.kind === "people" && route.tab === "history")) expect(controls.length, "the page shows its controls").toBeGreaterThan(0);
    for (const el of controls) {
      const name = called(el);
      // ci is bo's: its tokens are theirs, as the server's owner rule has it.
      if (/^(make a token for ci|revoke ci's token laptop)$/.test(name)) {
        expect(el.matches(":disabled"), `${name}, theirs`).toBe(false);
        continue;
      }
      expect(el.matches(":disabled"), `${name} is disabled`).toBe(true);
      const why = el.closest("[title]")?.getAttribute("title") ?? "";
      expect(why, `${name} says why`).toMatch(/^bo may not do that: /);
    }
  });
});

describe("People and access, to an admin", () => {
  it.each(PAGES.map((r) => [JSON.stringify(r), r] as const))("%s: each row's control is there to use", async (_, route) => {
    vi.mocked(mayI).mockResolvedValue(ADMIN);
    await show(route);
    for (const el of sending().filter((el) => !el.closest("form"))) expect(el.matches(":disabled"), called(el)).toBe(false);
  });
});
