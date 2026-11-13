// The buttons follow `can` (can.ts, the access plan's step 7c), held to what the server answers:
// can.fixture.json is written by the server's own test (scopes.rs's
// `the_editor_is_held_to_what_can_answers`, HOLON_BLESS=1) from its catalogue, its route table and
// what `/auth/me/can` answers five callers. So the catalogue is the server's, each button asks the
// action its route asks, and the table below says, for each caller, which buttons they may use and
// which are disabled in the server's words. Then the hook: one read per flow however many follow
// it, kept until the access version moves, a change is made or refused, or someone else signs in;
// and a choice made outside a render (deploying under a name just typed) reads from the same.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import fixture from "./can.fixture.json";
import { ACTIONS, BUTTONS, canOn, checkVersion, gate, refusal, resetCan, useCan, type Button, type Can } from "./can";
import { mayHaveChanged } from "./server";

type Caller = keyof typeof fixture.callers;
const CALLERS: Caller[] = ["admin", "editor", "viewer", "point", "agent"];
const answer = (who: Caller, at: "host" | "flow") => fixture.callers[who][at] as Can;

describe("what the buttons ask", () => {
  it("the catalogue is the server's, in its order", () => {
    expect([...ACTIONS]).toEqual(fixture.actions);
  });

  it("each button asks what its route asks, or, inside, an action of the catalogue its handler asks", () => {
    const routes = fixture.routes as Record<string, string>;
    for (const [name, b] of Object.entries(BUTTONS) as [Button, (typeof BUTTONS)[Button]][]) {
      expect(routes[b.route], `${name}: ${b.route} is a route the server answers, asking can`).toBeDefined();
      if ("inside" in b) expect(ACTIONS).toContain(b.action);
      else expect(b.action, `${name}: ${b.route}`).toBe(routes[b.route]);
    }
  });
});

/** Each button, where it asks (its flow's answer, or the one with no flow: the host's, and the
 *  others on any flow), and which of admin, editor, viewer, point (bound at d's approval point
 *  alone) and agent may use it: ✓, or · for refused. */
const TABLE: [Button, "host" | "flow", string][] = [
  //                                  admin editor viewer point agent
  ["deploy", "flow", /*            */ "✓ ✓ · · ✓"],
  ["approve", "flow", /*           */ "✓ ✓ · · ·"],
  ["run", "flow", /*               */ "✓ ✓ · · ·"],
  ["pause", "flow", /*             */ "✓ ✓ · · ·"],
  ["stop", "flow", /*              */ "✓ ✓ · · ·"],
  ["remove", "flow", /*            */ "✓ ✓ · · ·"],
  ["deleteSaved", "host", /*       */ "✓ ✓ · · ·"],
  ["decline", "flow", /*           */ "✓ ✓ · · ·"],
  ["grant", "flow", /*             */ "✓ ✓ · · ·"],
  ["agentsPropose", "flow", /*     */ "✓ ✓ · · ·"],
  ["setSecret", "flow", /*         */ "✓ · · · ·"],
  ["removeSecret", "flow", /*      */ "✓ · · · ·"],
  ["rollBackSecret", "flow", /*    */ "✓ · · · ·"],
  ["setServerSecret", "host", /*   */ "✓ · · · ·"],
  ["removeServerSecret", "host", /**/ "✓ · · · ·"],
  ["rollBackServerSecret", "host", "✓ · · · ·"],
  ["capServerSecret", "host", /*   */ "✓ · · · ·"],
  ["reloadPolicy", "host", /*      */ "✓ ✓ · · ·"],
  ["runAction", "flow", /*         */ "✓ ✓ · · ✓"],
  ["bind", "host", /*              */ "✓ · · · ·"],
  ["unbind", "host", /*            */ "✓ · · · ·"],
  ["people", "host", /*            */ "✓ · · · ·"],
  ["resources", "host", /*         */ "✓ · · · ·"],
  ["policy", "host", /*            */ "✓ ✓ ✓ · ✓"],
];

describe("each caller's buttons, as the server answers them", () => {
  it("covers every button", () => {
    expect(TABLE.map(([b]) => b).sort()).toEqual(Object.keys(BUTTONS).sort());
  });

  it.each(CALLERS.map((who, i) => [who, i] as const))("%s: may use those marked, and sees the others disabled in the server's words", (who, i) => {
    for (const [button, at, marks] of TABLE) {
      const can = answer(who, at);
      const may = marks.split(" ")[i] === "✓";
      const said = refusal(can, button);
      expect(said === undefined, `${who}: ${button}`).toBe(may);
      const shown = gate(can, button, false, "what it does");
      if (may) expect(shown, `${who}: ${button}`).toEqual({ disabled: false, title: "what it does" });
      else {
        expect(shown, `${who}: ${button}`).toEqual({ disabled: true, title: can[BUTTONS[button].action] });
        expect(said, `${who}: ${button}`).toMatch(/may not do that: /);
      }
    }
  });

  it("before the server has answered, nothing is disabled that wasn't already", () => {
    expect(gate(null, "pause", false, "stop answering")).toEqual({ disabled: false, title: "stop answering" });
    expect(gate(null, "deploy", true)).toEqual({ disabled: true, title: undefined });
  });
});

describe("what tells the buttons to ask again", () => {
  it.each([
    ["http://127.0.0.1:7420/bindings", "POST", 200, true],
    ["http://127.0.0.1:7420/bindings/7?why=x", "DELETE", 204, true],
    ["http://127.0.0.1:7420/deploy", "POST", 200, true],
    ["http://127.0.0.1:7420/deployments/d/pause", "POST", 403, true],
    ["http://127.0.0.1:7420/deployments/d/pause", "POST", 500, false],
    ["http://127.0.0.1:7420/deployments", "GET", 403, false],
    ["http://127.0.0.1:7420/auth/prefs", "POST", 204, false],
    ["http://127.0.0.1:7420/auth/password", "POST", 403, false],
    ["http://127.0.0.1:7420/unit", "POST", 200, false],
    ["http://127.0.0.1:7420/deploy/preview", "POST", 200, false],
    ["/roles/keeper", "PUT", 200, true],
  ] as const)("%s %s %i: %s", (url, method, status, said) => {
    expect(mayHaveChanged(url, method, status)).toBe(said);
  });
});

// ── the hook ─────────────────────────────────────────────────────────────────────────────────
beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

/** The server, as far as the hook reads it: `/auth/me/can` answers `answers[flow]` (each call
 *  counted), `/auth/me` the access version `version`. */
let answers: Record<string, Can> = {};
let version = 1;
let asked: string[] = [];
let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  resetCan();
  answers = { "": { "people.view": "cy may not do that: that needs an admin" }, d: { "flow.lifecycle": true } };
  version = 1;
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      const u = new URL(url, "http://127.0.0.1:7420");
      const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
      if (u.pathname === "/auth/me/can") {
        const flow = u.searchParams.get("flow") ?? "";
        asked.push(flow);
        expect(u.searchParams.get("actions")?.split(",")).toEqual([...ACTIONS]);
        return json(answers[flow] ?? {});
      }
      if (u.pathname === "/auth/me") return json({ name: "cy", kind: "person", role: "viewer", access_version: version });
      return new Response("", { status: 404 });
    }),
  );
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  root = null;
  vi.unstubAllGlobals();
});

/** What each `useCan(flow)` shown answered, by flow. */
const shown: Record<string, Can | null> = {};
function Shows({ flows }: { flows: (string | null)[] }) {
  return (
    <>
      {flows.map((f, i) => (
        <One key={i} flow={f} />
      ))}
    </>
  );
}
function One({ flow }: { flow: string | null }) {
  shown[flow ?? ""] = useCan(flow);
  return null;
}

/** Every pending promise settled, and what they set rendered. */
const settle = () => act(async () => {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
});

async function mount(flows: (string | null)[]) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Shows flows={flows} />));
  await settle();
}

describe("useCan", () => {
  it("reads each flow once, however many buttons follow it, every action at once", async () => {
    await mount(["d", "d", null, "d"]);
    expect(asked.sort()).toEqual(["", "d"]);
    expect(shown.d).toEqual({ "flow.lifecycle": true });
    expect(shown[""]).toEqual({ "people.view": "cy may not do that: that needs an admin" });
  });

  it("a change made or refused here reads it again, showing what was kept meanwhile", async () => {
    await mount(["d"]);
    answers.d = { "flow.lifecycle": "cy may not do that: a viewer only reads" };
    act(() => void window.dispatchEvent(new Event("holon:can")));
    expect(shown.d).toEqual({ "flow.lifecycle": true });
    await settle();
    expect(asked).toEqual(["d", "d"]);
    expect(shown.d).toEqual({ "flow.lifecycle": "cy may not do that: a viewer only reads" });
  });

  it("the access version moving reads it again; the same version doesn't", async () => {
    await mount(["d"]);
    await act(() => checkVersion());
    expect(asked).toEqual(["d"]);
    version = 2;
    answers.d = {};
    await act(() => checkVersion());
    await settle();
    expect(asked).toEqual(["d", "d"]);
    expect(shown.d).toEqual({});
  });

  it("a choice made outside a render takes what is kept, else reads it once and keeps it for the buttons", async () => {
    await mount(["d"]);
    expect(await canOn("d")).toEqual({ "flow.lifecycle": true });
    expect(asked).toEqual(["d"]);
    answers.e = { "flow.approve-deploy": "cy may not do that: a viewer only reads" };
    expect(await canOn("e")).toEqual(answers.e);
    expect(await canOn("e")).toEqual(answers.e);
    expect(asked).toEqual(["d", "e"]);
    act(() => void window.dispatchEvent(new Event("holon:can")));
    await settle();
    expect(await canOn("e")).toEqual(answers.e);
    expect(asked.filter((f) => f === "e")).toHaveLength(2);
  });

  it("someone else signing in keeps nothing of the one before", async () => {
    await mount(["d"]);
    answers.d = { "flow.lifecycle": "bob may not do that: a viewer only reads" };
    act(() => void window.dispatchEvent(new Event("holon:session")));
    expect(shown.d).toBeNull();
    await settle();
    expect(shown.d).toEqual({ "flow.lifecycle": "bob may not do that: a viewer only reads" });
  });
});
