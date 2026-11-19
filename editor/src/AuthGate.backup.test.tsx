// Set up, the first admin is asked where the runs a flow's log lets go of go, while the host warns
// that no one has chosen (AuthGate.tsx's BackupStep, storage.rs `backup-choice`). Each choice is
// written as `holon policy --backup` writes it (POST /admin/storage): a folder, or none on
// purpose; later writes nothing and leaves the warning. A host that doesn't warn (chosen, or a
// policy that is no file) isn't asked, and neither is anyone signing in or joining: they meet the
// question in the warning.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthGate } from "./AuthGate";
import { getAuthState, getWarnings, setStorage, setUp, type HostWarning } from "./server";

vi.mock("./server", async (original) => {
  const real = await original<typeof import("./server")>();
  return { ...real, getAuthState: vi.fn(), setUp: vi.fn(), getWarnings: vi.fn(), setStorage: vi.fn() };
});

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

const NOT_CHOSEN: HostWarning = { key: "backup-choice", says: "no backup folder is chosen", todo: "choose" };
const OTHER: HostWarning = { key: "disk:/data", says: "the disk is full", todo: "make room" };

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  vi.mocked(getAuthState).mockResolvedValue({ setup: true, oidc: null } as Awaited<ReturnType<typeof getAuthState>>);
  vi.mocked(setUp).mockResolvedValue({ name: "ada", kind: "person", role: "admin" } as Awaited<ReturnType<typeof setUp>>);
  vi.mocked(setStorage).mockResolvedValue({} as Awaited<ReturnType<typeof setStorage>>);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

const settle = () => act(async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
});
const form = (name: string) => container.querySelector(`form[aria-label="${name}"]`);
const button = (f: Element | null, name: string) => [...(f?.querySelectorAll("button") ?? [])].find((b) => b.textContent?.trim() === name) as HTMLButtonElement | undefined;
function type(label: string, value: string) {
  const field = [...container.querySelectorAll("label")].find((l) => l.firstChild?.textContent?.trim() === label)?.querySelector("input");
  if (!field) throw new Error(`no field ${label}`);
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

/** The console opened on a new server, its admin set up: what it shows next. */
async function setUpAda(warnings: HostWarning[] | Error) {
  if (warnings instanceof Error) vi.mocked(getWarnings).mockRejectedValue(warnings);
  else vi.mocked(getWarnings).mockResolvedValue(warnings);
  await act(async () => root.render(<AuthGate>{(me) => <div>editor of {me.name}</div>}</AuthGate>));
  await settle();
  const setup = form("set up");
  expect(setup, "the setup form").not.toBeNull();
  await act(async () => {
    type("setup code", "CODE-1234");
    type("your name", "ada");
    type("password (at least 10 characters)", "a-long-password");
    type("the password again", "a-long-password");
  });
  await act(async () => button(setup, "set up")!.click());
  await settle();
}

describe("the first admin, set up, is asked where runs go", () => {
  it("while the host warns that no one has chosen: a folder is written, and the editor shows", async () => {
    await setUpAda([OTHER, NOT_CHOSEN]);
    const step = form("where runs go");
    expect(step, "asked").not.toBeNull();
    expect(container.textContent).not.toContain("editor of ada");
    // Nothing said: not a choice.
    await act(async () => button(step, "keep them in this folder")!.click());
    await settle();
    expect(step!.querySelector("[role=alert]")?.textContent).toBe("say a folder, or choose none");
    expect(setStorage).not.toHaveBeenCalled();
    await act(async () => type("a folder on the server's machine", "  /mnt/archive  "));
    await act(async () => button(step, "keep them in this folder")!.click());
    await settle();
    expect(vi.mocked(setStorage).mock.calls).toEqual([[{ backup: "/mnt/archive" }]]);
    expect(container.textContent).toContain("editor of ada");
  });

  it("none, on purpose: written as none, and the editor shows", async () => {
    await setUpAda([NOT_CHOSEN]);
    await act(async () => button(form("where runs go"), "none: delete them")!.click());
    await settle();
    expect(vi.mocked(setStorage).mock.calls).toEqual([[{ backup: "none" }]]);
    expect(container.textContent).toContain("editor of ada");
  });

  it("later: nothing written, the warning left, and the editor shows", async () => {
    await setUpAda([NOT_CHOSEN]);
    await act(async () => button(form("where runs go"), "later")!.click());
    await settle();
    expect(setStorage).not.toHaveBeenCalled();
    expect(container.textContent).toContain("editor of ada");
  });

  it("a folder the server refuses: said as it says, and still asked", async () => {
    await setUpAda([NOT_CHOSEN]);
    vi.mocked(setStorage).mockRejectedValue(new Error('"s3://bucket": object storage is to come'));
    const step = form("where runs go");
    await act(async () => type("a folder on the server's machine", "s3://bucket"));
    await act(async () => button(step, "keep them in this folder")!.click());
    await settle();
    expect(step!.querySelector("[role=alert]")?.textContent).toContain("object storage is to come");
    expect(form("where runs go")).not.toBeNull();
    expect(container.textContent).not.toContain("editor of ada");
  });

  it("not asked where the host doesn't warn of it, or its warnings can't be read", async () => {
    for (const warnings of [[], [OTHER], new Error("the server can't be reached")]) {
      await setUpAda(warnings);
      expect(form("where runs go"), `${JSON.stringify(warnings)}: not asked`).toBeNull();
      expect(container.textContent).toContain("editor of ada");
      act(() => root.unmount());
      root = createRoot(container);
    }
    expect(setStorage).not.toHaveBeenCalled();
  });
});
