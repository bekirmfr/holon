// A toast says something once: a notice fades by itself, a failure stays until dismissed, the
// same text twice is one toast, only the newest few are shown, and one may link to a place.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { ToastProvider, TOAST_MS, useToast, type Notify } from "./Toast";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  vi.useRealTimers();
});

/** Mount a provider, and hand back what it says things with. */
function mount(): Notify {
  container = document.createElement("div");
  document.body.appendChild(container);
  let notify: Notify = () => {};
  function Grab() {
    notify = useToast();
    return null;
  }
  root = createRoot(container);
  act(() => root!.render(<ToastProvider>{<Grab />}</ToastProvider>));
  return (text, tone, link) => act(() => notify(text, tone, link));
}

const shown = () => [...document.querySelectorAll(".hl-toast__text")].map((e) => e.textContent);

describe("toasts", () => {
  it("fades a notice after a while, and keeps a failure until it is dismissed", () => {
    vi.useFakeTimers();
    const say = mount();
    say("saved 12:00:01");
    say("not saved: the server is away", "bad");
    expect(shown()).toEqual(["saved 12:00:01", "not saved: the server is away"]);
    expect(document.querySelector(".hl-toast--ok")?.getAttribute("role")).toBe("status");
    expect(document.querySelector(".hl-toast--bad")?.getAttribute("role")).toBe("alert");
    act(() => vi.advanceTimersByTime(TOAST_MS + 1));
    expect(shown()).toEqual(["not saved: the server is away"]);
    act(() => document.querySelector<HTMLButtonElement>(".hl-toast__close")!.click());
    expect(shown()).toEqual([]);
  });

  it("says the same thing once, and shows only the newest few", () => {
    const say = mount();
    say("saved");
    say("saved");
    expect(shown()).toEqual(["saved"]);
    for (const n of [1, 2, 3, 4, 5]) say(`step ${n}`);
    expect(shown()).toEqual(["step 2", "step 3", "step 4", "step 5"]);
  });

  it("says nothing for an empty text, and nothing at all without a provider", () => {
    const say = mount();
    say("");
    expect(shown()).toEqual([]);
    let outside: Notify | null = null;
    function Lone() {
      outside = useToast();
      return null;
    }
    const other = document.createElement("div");
    const r = createRoot(other);
    act(() => r.render(<Lone />));
    expect(() => outside!("anything")).not.toThrow();
    act(() => r.unmount());
  });

  it("links to where it can be seen, and goes once followed", () => {
    const say = mount();
    say("Deployed refunds.", "ok", { label: "Open the flow", to: { kind: "flow", slug: "refunds", tab: "status" } });
    const link = document.querySelector<HTMLAnchorElement>(".hl-toast a")!;
    expect(link.textContent).toBe("Open the flow");
    expect(link.getAttribute("href")).toBe("#refunds");
    act(() => link.click());
    expect(shown()).toEqual([]);
  });
});
