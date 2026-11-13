// A question in the page answers to keys as well as clicks: Enter is its button, Esc cancels, and
// a yes/no question takes y and n, shown as chips that stay out of the buttons' names. Keys that
// answer it don't reach the page beneath; typing in a prompt still reaches its field. A notice has
// one button, and its letter.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { DialogProvider, useDialog } from "./Dialog";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

type Dialogs = ReturnType<typeof useDialog>;

function mount(): Dialogs {
  container = document.createElement("div");
  document.body.appendChild(container);
  let dialogs: Dialogs | null = null;
  function Grab() {
    dialogs = useDialog();
    return null;
  }
  root = createRoot(container);
  act(() => root!.render(<DialogProvider>{<Grab />}</DialogProvider>));
  return dialogs!;
}

/** Open a question, and hand back its answer to come (wrapped: an async function returning the
 *  promise itself would wait for it). */
async function ask<T>(open: () => Promise<T>): Promise<{ answer: Promise<T> }> {
  let answer!: Promise<T>;
  await act(async () => {
    answer = open();
  });
  return { answer };
}

function press(key: string, init: KeyboardEventInit = {}, target: EventTarget = document.body) {
  act(() => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  });
}

const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("[role=dialog] button")];
/** A button's name as assistive technology reads it: its text without what is aria-hidden. */
const nameOf = (b: HTMLElement) =>
  [...b.childNodes]
    .filter((n) => !(n instanceof HTMLElement && n.getAttribute("aria-hidden") === "true"))
    .map((n) => n.textContent)
    .join("")
    .trim();

describe("dialog keys", () => {
  it("answers yes with y and no with n, and shows both as chips outside the buttons' names", async () => {
    const d = mount();
    const { answer: yes } = await ask(() => d.confirm("Deploy it?", "Approve and deploy"));
    expect(buttons().map(nameOf)).toEqual(["Cancel", "Approve and deploy"]);
    expect(buttons().map((b) => b.querySelector("kbd")?.textContent)).toEqual(["n", "y"]);
    press("y");
    await expect(yes).resolves.toBe(true);
    const { answer: no } = await ask(() => d.confirm("Remove it?", "Remove"));
    press("n");
    await expect(no).resolves.toBe(false);
  });

  it("leaves Ctrl+N and the like to the browser", async () => {
    const d = mount();
    const { answer } = await ask(() => d.confirm("Discard?", "Discard"));
    press("n", { ctrlKey: true });
    expect(document.querySelector("[role=dialog]")).not.toBeNull();
    press("Escape");
    await expect(answer).resolves.toBe(false);
  });

  it("keeps the keys that answer it from the page beneath", async () => {
    const d = mount();
    const seen: string[] = [];
    const listen = (e: KeyboardEvent) => seen.push(e.key);
    window.addEventListener("keydown", listen);
    const { answer } = await ask(() => d.confirm("Update it?", "Update"));
    press("y");
    await expect(answer).resolves.toBe(true);
    window.removeEventListener("keydown", listen);
    expect(seen).toEqual([]);
  });

  it("lets a prompt's field have its letters, and answers it with Enter", async () => {
    const d = mount();
    const { answer: name } = await ask(() => d.prompt("Name this flow first:"));
    const field = document.querySelector<HTMLInputElement>("[aria-modal=true] input")!;
    expect(buttons().some((b) => b.querySelector("kbd"))).toBe(false);
    let typed = 0;
    field.addEventListener("keydown", () => typed++);
    press("n", {}, field);
    expect(typed).toBe(1);
    expect(document.querySelector("[role=dialog]")).not.toBeNull();
    act(() => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(field, "  shout ");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    press("Enter", {}, field);
    await expect(name).resolves.toBe("shout");
  });
  it("gives each of a choice's answers its letter, the last answered by Enter too, Esc none", async () => {
    const d = mount();
    const three = ["Keep it open", "Discard", "Save and close"];
    const { answer: kept } = await ask(() => d.choose("Close it?", three));
    expect(buttons().map((b) => b.querySelector("kbd")?.textContent)).toEqual(["k", "d", "s"]);
    press("k");
    await expect(kept).resolves.toBe(0);
    const { answer: saved } = await ask(() => d.choose("Close it?", three));
    press("Enter");
    await expect(saved).resolves.toBe(2);
    const { answer: none } = await ask(() => d.choose("Close it?", three));
    press("Escape");
    await expect(none).resolves.toBeNull();
  });

  it("shows a notice with its one button, closed by its letter, Enter or Esc", async () => {
    const d = mount();
    const { answer: lettered } = await ask(() => d.notice("The chain of shop's runs", "Close"));
    expect(buttons().map(nameOf)).toEqual(["Close"]);
    expect(buttons().map((b) => b.querySelector("kbd")?.textContent)).toEqual(["c"]);
    press("c");
    await expect(lettered).resolves.toBeUndefined();
    expect(document.querySelector("[role=dialog]")).toBeNull();
    for (const key of ["Enter", "Escape"]) {
      const { answer } = await ask(() => d.notice("Your password is changed."));
      expect(buttons().map(nameOf)).toEqual(["OK"]);
      press(key);
      await expect(answer).resolves.toBeUndefined();
    }
  });
});
