// Pages and buttons follow `can` (web-console-L4.md §3, principle 6; §13): a button the caller may
// not use is disabled, with the server's refusal as its title; a page asks once for all its
// buttons; and the nav's host group lists the pages the caller may view, for a viewer, an admin,
// and a person who owns an agent.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { hostEntries } from "./ui/Shell";
import { canOf, gateOf, Gated, hostPagesOf, useCan, useHostPages, type Answer, type HostPages } from "./useCan";
import { listOwned, mayI, setSession, type Principal } from "./server";

vi.mock("./server", async (original) => ({ ...(await original<typeof import("./server")>()), mayI: vi.fn(), listOwned: vi.fn() }));

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
beforeEach(() => {
  // A new sign-in: nothing kept from the test before.
  setSession(null);
  vi.mocked(mayI).mockReset();
  vi.mocked(listOwned).mockReset();
});
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

async function mount(node: React.ReactNode) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => root!.render(node));
}

const button = (name: string) => [...document.querySelectorAll("button")].find((b) => b.textContent === name)!;

/** What the server answers a viewer, an admin and an editor for the host's pages (can.rs: the
 *  built-in viewer and editor hold policy.view and none of the host's other views). */
const NEEDS_ADMIN = "ada may not do that: that needs an admin";
const VIEWER: Record<string, Answer> = { "resource.view": NEEDS_ADMIN, "people.view": NEEDS_ADMIN, "policy.view": true, "host.view": NEEDS_ADMIN };
const ADMIN: Record<string, Answer> = { "resource.view": true, "people.view": true, "policy.view": true, "host.view": true };
const EDITOR = VIEWER;

describe("a button the caller may not use", () => {
  it("is disabled, with the refusal as its title; one they may use keeps its own", () => {
    expect(gateOf("a viewer only reads", { title: "unbind the name" })).toEqual({ disabled: true, title: "a viewer only reads" });
    expect(gateOf(true, { title: "unbind the name" })).toEqual({ disabled: false, title: "unbind the name" });
    // Its own reason to be disabled stays.
    expect(gateOf(true, { disabled: true })).toEqual({ disabled: true, title: undefined });
  });

  it("is usable until the server answers, and shown only to whoever holds it once it has", () => {
    const asking = canOf(null);
    expect(asking.may("resource.bind")).toBe(true);
    expect(asking.holds("resource.view")).toBe(false);
    const told = canOf({ "resource.bind": "no", "resource.view": true });
    expect(told.may("resource.bind")).toBe("no");
    expect(told.holds("resource.view")).toBe(true);
  });

  it("is asked of the server once for the whole page, and its form's controls are disabled with the refusal", async () => {
    vi.mocked(mayI).mockResolvedValue({ "resource.bind": NEEDS_ADMIN, "resource.unbind": true });
    function Page() {
      const can = useCan(["resource.bind", "resource.unbind"]);
      return (
        <>
          <button {...can.gate("resource.unbind", { title: "unbind the name" })}>unbind</button>
          <Gated may={can.may("resource.bind")}>
            <form>
              <input aria-label="name" />
              <button type="submit">bind</button>
            </form>
          </Gated>
        </>
      );
    }
    await mount(<Page />);
    expect(mayI).toHaveBeenCalledTimes(1);
    expect(vi.mocked(mayI).mock.calls[0][0].sort()).toEqual(["resource.bind", "resource.unbind"]);
    expect(button("unbind").disabled).toBe(false);
    expect(button("unbind").title).toBe("unbind the name");
    const fieldset = document.querySelector("fieldset")!;
    expect(fieldset.disabled).toBe(true);
    expect(fieldset.title).toBe(NEEDS_ADMIN);
    expect(button("bind").matches(":disabled")).toBe(true);
    expect(document.querySelector("input")!.matches(":disabled")).toBe(true);
  });

  it("asks on the flow its page is of, and keeps the answer a while for the next page that asks", async () => {
    vi.mocked(mayI).mockResolvedValue({ "flow.lifecycle": "ada may not do that: a viewer only reads" });
    function Header() {
      const can = useCan(["flow.lifecycle"], { flow: "payout" });
      return <button {...can.gate("flow.lifecycle")}>Stop</button>;
    }
    await mount(<Header />);
    expect(vi.mocked(mayI).mock.calls[0][1]).toEqual({ flow: "payout" });
    expect(button("Stop").disabled).toBe(true);
    expect(button("Stop").title).toBe("ada may not do that: a viewer only reads");
    act(() => root?.unmount());
    await mount(<Header />);
    expect(mayI).toHaveBeenCalledTimes(1);
    expect(button("Stop").disabled).toBe(true);
  });
});

describe("the nav's host group", () => {
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
    vi.mocked(mayI).mockResolvedValue(EDITOR);
    const agent = { name: "ada-agent", kind: "agent", owner: "ada" } as Principal;
    let pages: HostPages | null = null;
    function Nav({ name }: { name: string }) {
      pages = useHostPages({ name, kind: "person" });
      return null;
    }
    vi.mocked(listOwned).mockResolvedValue([agent]);
    await mount(<Nav name="ada" />);
    expect(labels(pages!)).toEqual(["Your agents and services", "Policy"]);
    act(() => root?.unmount());
    vi.mocked(listOwned).mockResolvedValue([]);
    await mount(<Nav name="bob" />);
    expect(labels(pages!)).toEqual(["Policy"]);
  });
});
