// The buttons and pages follow `can` (can.tsx; web-console-L4.md §3, principle 6; the access plan's
// step 7c), held to what the server answers: can.fixture.json is written by the server's own test
// (scopes.rs's `the_editor_is_held_to_what_can_answers`, HOLON_BLESS=1) from its catalogue, its
// route table and what `/auth/me/can` answers five callers. So the catalogue is the server's, each
// button asks the action its route asks, and the table below says, for each caller, which buttons
// they may use and which are disabled in the server's words. Then the hook: one read per flow
// however many follow it, kept until the access version moves, a change is made or refused, a
// minute goes by, or someone else signs in; a choice made outside a render (deploying under a name
// just typed) reads from the same; and the nav's host group lists the pages the caller may view.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import fixture from "./can.fixture.json";
import { ACTIONS, AGAIN_EVERY_MS, ASKING, BUTTONS, canOf, canOn, checkVersion, Gated, gateOf, hostPagesOf, resetCan, useCan, useHostPages, type Answers, type Button, type Can, type HostPages } from "./can";
import { mayHaveChanged } from "./server";
import { hostEntries } from "./ui/Shell";

type Caller = keyof typeof fixture.callers;
const CALLERS: Caller[] = ["admin", "editor", "viewer", "point", "agent"];
const answer = (who: Caller, at: "host" | "flow") => fixture.callers[who][at] as Answers;

describe("what the buttons ask", () => {
  it("the catalogue is the server's, in its order", () => {
    expect([...ACTIONS]).toEqual(fixture.actions);
  });

  it("each button asks what its routes ask, or, inside, an action of the catalogue its handler asks", () => {
    // A route asking two actions (can.rs's `Rule::Both`) lists them, the first asked first.
    const routes = fixture.routes as Record<string, string | string[]>;
    for (const [name, b] of Object.entries(BUTTONS) as [Button, (typeof BUTTONS)[Button]][]) {
      for (const route of [b.route].flat()) {
        expect(routes[route], `${name}: ${route} is a route the server answers, asking can`).toBeDefined();
        const asks = "also" in b ? [b.action, b.also] : [b.action];
        if ("inside" in b) expect(ACTIONS).toContain(b.action);
        else expect(asks, `${name}: ${route}`).toEqual([routes[route]].flat());
      }
    }
  });
});

/** Each button, where it asks (its flow's answer, or the one with no flow: the host's, and the
 *  others on any flow), and which of admin, editor, viewer, point (bound at d's approval point
 *  alone) and agent may use it: ✓, or · for refused. */
const TABLE: [Button, "host" | "flow", string][] = [
  //                                  admin editor viewer point agent
  ["save", "host", /*              */ "✓ ✓ · · ✓"],
  ["deploy", "flow", /*            */ "✓ ✓ · · ✓"],
  ["deployNow", "flow", /*         */ "✓ ✓ · · ·"],
  ["approve", "flow", /*           */ "✓ ✓ · · ·"],
  ["decline", "flow", /*           */ "✓ ✓ · · ·"],
  ["request", "flow", /*           */ "✓ ✓ · · ✓"],
  ["runAction", "flow", /*         */ "✓ ✓ · · ✓"],
  ["tick", "flow", /*              */ "✓ ✓ · · ·"],
  ["run", "flow", /*               */ "✓ ✓ · · ·"],
  ["pause", "flow", /*             */ "✓ ✓ · · ·"],
  ["stop", "flow", /*              */ "✓ ✓ · · ·"],
  ["remove", "flow", /*            */ "✓ ✓ · · ·"],
  ["grant", "flow", /*             */ "✓ ✓ · · ·"],
  ["settings", "flow", /*          */ "✓ ✓ · · ·"],
  ["restore", "flow", /*           */ "✓ ✓ · · ·"],
  ["export", "flow", /*            */ "✓ ✓ ✓ · ·"],
  ["setSecret", "flow", /*         */ "✓ · · · ·"],
  ["removeSecret", "flow", /*      */ "✓ · · · ·"],
  ["rollBackSecret", "flow", /*    */ "✓ · · · ·"],
  ["deleteSaved", "host", /*       */ "✓ ✓ · · ·"],
  ["bindResource", "host", /*      */ "✓ · · · ·"],
  ["unbindResource", "host", /*    */ "✓ · · · ·"],
  ["setServerSecret", "host", /*   */ "✓ · · · ·"],
  ["removeServerSecret", "host", /**/ "✓ · · · ·"],
  ["rollBackServerSecret", "host", "✓ · · · ·"],
  ["capServerSecret", "host", /*   */ "✓ · · · ·"],
  ["operate", "host", /*           */ "✓ · · · ·"],
  ["forgetKeys", "host", /*        */ "✓ · · · ·"],
  ["check", "host", /*             */ "✓ ✓ · · ·"],
  ["reloadPolicy", "host", /*      */ "✓ ✓ · · ·"],
  ["addPrincipal", "host", /*      */ "✓ · · · ·"],
  ["giveRole", "host", /*          */ "✓ · · · ·"],
  ["changePrincipal", "host", /*   */ "✓ · · · ·"],
  ["resetPerson", "host", /*       */ "✓ · · · ·"],
  ["removePrincipal", "host", /*   */ "✓ · · · ·"],
  ["tokens", "host", /*            */ "✓ · · · ·"],
  ["oidc", "host", /*              */ "✓ · · · ·"],
  ["groups", "host", /*            */ "✓ · · · ·"],
  ["roles", "host", /*             */ "✓ · · · ·"],
  ["bind", "host", /*              */ "✓ · · · ·"],
  ["unbind", "host", /*            */ "✓ · · · ·"],
  ["approveAccess", "host", /*     */ "✓ · · · ·"],
  ["declineAccess", "host", /*     */ "✓ · · · ·"],
  ["resources", "host", /*         */ "✓ · · · ·"],
  ["people", "host", /*            */ "✓ · · · ·"],
  ["policy", "host", /*            */ "✓ ✓ ✓ · ✓"],
  ["host", "host", /*              */ "✓ · · · ·"],
];

describe("each caller's buttons, as the server answers them", () => {
  it("covers every button", () => {
    expect(TABLE.map(([b]) => b).sort()).toEqual(Object.keys(BUTTONS).sort());
  });

  it.each(CALLERS.map((who, i) => [who, i] as const))("%s: may use those marked, and sees the others disabled in the server's words", (who, i) => {
    for (const [button, at, marks] of TABLE) {
      const answers = answer(who, at);
      const can = canOf(answers);
      const may = marks.split(" ")[i] === "✓";
      expect(can.may(button) === true, `${who}: ${button}`).toBe(may);
      expect(can.holds(button), `${who}: ${button}`).toBe(may);
      const shown = can.gate(button, { title: "what it does" });
      if (may) expect(shown, `${who}: ${button}`).toEqual({ disabled: false, title: "what it does" });
      else {
        expect(shown, `${who}: ${button}`).toEqual({ disabled: true, title: answers[BUTTONS[button].action] });
        // A run's steps are refused in words of their own, saying who gives them (R55).
        expect(can.may(button), `${who}: ${button}`).toMatch(/may not do that: |may not read .+ run\.steps; a person gives it/);
      }
    }
  });

  it("a control's own reasons to be disabled stay, and so does its own title while it may", () => {
    expect(gateOf("a viewer only reads", { title: "unbind the name" })).toEqual({ disabled: true, title: "a viewer only reads" });
    expect(gateOf(true, { title: "unbind the name" })).toEqual({ disabled: false, title: "unbind the name" });
    expect(gateOf(true, { disabled: true })).toEqual({ disabled: true, title: undefined });
  });

  it("before the server has answered, every button is disabled, saying it is asking, and nothing is shown only to whoever holds it", () => {
    const asking = canOf(null);
    expect(asking.known).toBe(false);
    for (const button of Object.keys(BUTTONS) as Button[]) expect(asking.gate(button, { title: "its own" }), button).toEqual({ disabled: true, title: ASKING });
    expect(asking.holds("resources")).toBe(false);
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

const NEEDS_ADMIN = "cy may not do that: that needs an admin";
const READS_ONLY = "cy may not do that: a viewer only reads";

/** The server, as far as the hook reads it: `/auth/me/can` answers `answers[flow]` (each call
 *  counted), `/auth/me` the access version `version`, `/auth/owned` what `owned` names. */
let answers: Record<string, Answers> = {};
let version = 1;
let owned: string[] = [];
let asked: string[] = [];
/** How many reads of `/auth/me` to come get no answer: held up, as a busy browser held them. */
let meHeld = 0;
let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  resetCan();
  answers = { "": { "people.view": NEEDS_ADMIN }, d: { "flow.lifecycle": true } };
  version = 1;
  owned = [];
  asked = [];
  meHeld = 0;
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
      if (u.pathname === "/auth/me" && meHeld > 0) {
        meHeld -= 1;
        return new Promise<Response>(() => undefined);
      }
      if (u.pathname === "/auth/me") return json({ name: "cy", kind: "person", role: "viewer", access_version: version });
      if (u.pathname === "/auth/owned") return json(owned.map((name) => ({ name, kind: "agent", owner: "cy" })));
      return new Response("", { status: 404 });
    }),
  );
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  root = null;
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** What each `useCan(flow)` shown answered, by flow. */
const shown: Record<string, Can> = {};
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
const settle = () =>
  act(async () => {
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
    await new Promise((r) => setTimeout(r, 0));
  });

async function mount(node: React.ReactNode) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  await settle();
}

describe("useCan", () => {
  it("reads each flow once, however many buttons follow it, every action at once", async () => {
    await mount(<Shows flows={["d", "d", null, "d"]} />);
    expect(asked.sort()).toEqual(["", "d"]);
    expect(shown.d.may("pause")).toBe(true);
    expect(shown[""].may("people")).toBe(NEEDS_ADMIN);
  });

  it("disables a page's buttons, and its form's controls, with the refusal", async () => {
    answers[""] = { "resource.bind": NEEDS_ADMIN, "resource.unbind": true };
    function Page() {
      const can = useCan();
      return (
        <>
          <button {...can.gate("unbindResource", { title: "unbind the name" })}>unbind</button>
          <Gated may={can.may("bindResource")}>
            <form>
              <input aria-label="name" />
              <button type="submit">bind</button>
            </form>
          </Gated>
        </>
      );
    }
    await mount(<Page />);
    const button = (name: string) => [...document.querySelectorAll("button")].find((b) => b.textContent === name)!;
    expect(asked).toEqual([""]);
    expect(button("unbind").disabled).toBe(false);
    expect(button("unbind").title).toBe("unbind the name");
    const fieldset = document.querySelector("fieldset")!;
    expect(fieldset.disabled).toBe(true);
    expect(fieldset.title).toBe(NEEDS_ADMIN);
    expect(button("bind").matches(":disabled")).toBe(true);
    expect(document.querySelector("input")!.matches(":disabled")).toBe(true);
  });

  it("a change made or refused here reads it again, showing what was kept meanwhile", async () => {
    await mount(<Shows flows={["d"]} />);
    answers.d = { "flow.lifecycle": READS_ONLY };
    act(() => void window.dispatchEvent(new Event("holon:can")));
    expect(shown.d.may("pause")).toBe(true);
    await settle();
    expect(asked).toEqual(["d", "d"]);
    expect(shown.d.may("pause")).toBe(READS_ONLY);
  });

  it("the access version moving reads it again; the same version doesn't", async () => {
    await mount(<Shows flows={["d"]} />);
    await act(() => checkVersion());
    expect(asked).toEqual(["d"]);
    version = 2;
    answers.d = { "flow.lifecycle": READS_ONLY };
    await act(() => checkVersion());
    await settle();
    expect(asked).toEqual(["d", "d"]);
    expect(shown.d.may("stop")).toBe(READS_ONLY);
  });

  it("access moving between the reads at the start, the page follows: a first version read held up, a binding made, coming back to the window reads it again", async () => {
    // The page's first version read gets no answer; what the buttons follow is read meanwhile.
    meHeld = 1;
    await mount(<Shows flows={["d"]} />);
    expect(shown.d.may("pause")).toBe(true);
    // A binding takes the flow's lifecycle away: the version moves. The next version read, on
    // coming back to the window, is the first answered: what was kept was asked before it.
    version = 2;
    answers.d = { "flow.lifecycle": READS_ONLY };
    act(() => void window.dispatchEvent(new Event("focus")));
    await settle();
    expect(asked).toEqual(["d", "d"]);
    expect(shown.d.may("pause")).toBe(READS_ONLY);
    // Answered at once (the version read first, as the start asks it), what follows is kept:
    // the window coming back again reads nothing more.
    act(() => void window.dispatchEvent(new Event("focus")));
    await settle();
    expect(asked).toEqual(["d", "d"]);
  });

  it("a minute on, it reads it again however the version stands: a flow tagged by someone else's deploy shows", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    await mount(<Shows flows={["d"]} />);
    answers.d = { "flow.lifecycle": READS_ONLY };
    act(() => void vi.advanceTimersByTime(AGAIN_EVERY_MS));
    await settle();
    expect(asked).toEqual(["d", "d"]);
    expect(shown.d.may("run")).toBe(READS_ONLY);
  });

  it("a choice made outside a render takes what is kept, else reads it once and keeps it for the buttons", async () => {
    await mount(<Shows flows={["d"]} />);
    expect((await canOn("d")).may("pause")).toBe(true);
    expect(asked).toEqual(["d"]);
    answers.e = { "flow.approve-deploy": READS_ONLY };
    expect((await canOn("e")).may("approve")).toBe(READS_ONLY);
    expect((await canOn("e")).may("decline")).toBe(READS_ONLY);
    expect(asked).toEqual(["d", "e"]);
    act(() => void window.dispatchEvent(new Event("holon:can")));
    await settle();
    expect((await canOn("e")).may("approve")).toBe(READS_ONLY);
    expect(asked.filter((f) => f === "e")).toHaveLength(2);
  });

  it("someone else signing in keeps nothing of the one before", async () => {
    await mount(<Shows flows={["d"]} />);
    answers.d = { "flow.lifecycle": "bob may not do that: a viewer only reads" };
    act(() => void window.dispatchEvent(new Event("holon:session")));
    expect(shown.d.known).toBe(false);
    await settle();
    expect(shown.d.may("pause")).toBe("bob may not do that: a viewer only reads");
  });
});

describe("the nav's host group", () => {
  /** What the server answers a viewer, an admin and an editor for the host's pages (can.rs: the
   *  built-in viewer and editor hold policy.view and none of the host's other views). */
  const VIEWER: Answers = { "resource.view": NEEDS_ADMIN, "people.view": NEEDS_ADMIN, "policy.view": true, "host.view": NEEDS_ADMIN };
  const ADMIN: Answers = { "resource.view": true, "people.view": true, "policy.view": true, "host.view": true };
  const EDITOR = VIEWER;
  const labels = (pages: HostPages) => hostEntries(pages).map((e) => e.label);

  it("lists what each may view: a viewer the policy, an admin every page", () => {
    expect(labels(hostPagesOf(canOf(VIEWER), false))).toEqual(["Policy"]);
    expect(labels(hostPagesOf(canOf(ADMIN), false))).toEqual(["Resources", "People and access", "Policy"]);
    // Nothing until the server has said.
    expect(labels(hostPagesOf(canOf(null), false))).toEqual([]);
  });

  it("lists, to a person who owns an agent and may not see everyone, theirs", () => {
    expect(labels(hostPagesOf(canOf(EDITOR), true))).toEqual(["Your agents and services", "Policy"]);
    // One who may see everyone sees everyone, theirs among them.
    expect(labels(hostPagesOf(canOf(ADMIN), true))).toEqual(["Resources", "People and access", "Policy"]);
  });

  it("asks the server whether they own one: a person who owns an agent is shown theirs, one who owns none isn't", async () => {
    answers[""] = EDITOR;
    let pages: HostPages | null = null;
    function Nav({ name }: { name: string }) {
      pages = useHostPages({ name, kind: "person" });
      return null;
    }
    owned = ["cy-agent"];
    await mount(<Nav name="cy" />);
    expect(labels(pages!)).toEqual(["Your agents and services", "Policy"]);
    act(() => root?.unmount());
    owned = [];
    await mount(<Nav name="bob" />);
    expect(labels(pages!)).toEqual(["Policy"]);
  });
});
