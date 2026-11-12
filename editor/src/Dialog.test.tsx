// A question in the page answers to keys as well as clicks: Enter answers with what has the focus,
// Esc cancels, and a yes/no question takes y and n, shown as chips that stay out of the buttons'
// names. Keys that answer it don't reach the page beneath; typing in a prompt still reaches its
// field. (Tab staying in it is every modal's: modals.test.tsx.)

import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { DialogProvider, useDialog } from "./Dialog";
import { lap, press } from "./ui/pressKey";

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

/** The dialogs, over a page with a control of its own: where a Tab that left the dialog would go. */
function mount(): Dialogs {
  container = document.createElement("div");
  document.body.appendChild(container);
  let dialogs: Dialogs | null = null;
  function Grab() {
    dialogs = useDialog();
    return null;
  }
  root = createRoot(container);
  act(() =>
    root!.render(
      <DialogProvider>
        <button>the page's own button</button>
        <Grab />
      </DialogProvider>,
    ),
  );
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

const dialog = () => document.querySelector<HTMLElement>("[role=dialog]");
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("[role=dialog] button")];
/** A button's name as assistive technology reads it: its text without what is aria-hidden. */
const nameOf = (b: HTMLElement) =>
  [...b.childNodes]
    .filter((n) => !(n instanceof HTMLElement && n.getAttribute("aria-hidden") === "true"))
    .map((n) => n.textContent)
    .join("")
    .trim();
/** What a control is called in the table below: a button by its name, the prompt's field "field",
 *  anything else by its label. */
const called = (el: Element | null): string =>
  el instanceof HTMLButtonElement ? nameOf(el) : el instanceof HTMLInputElement ? "field" : (el?.getAttribute("aria-label") ?? String(el?.nodeName));

/** Enter on this control leaves the question open: it is not one of the controls that answer. */
const UNANSWERED = Symbol("unanswered");

/** A part with keys of its own in a confirm's body, as a deploy's review is: focusable, with a
 *  control inside that its closed rows hide (which no Tab may reach). */
const review: ReactNode = (
  <div role="group" aria-label="what changes" tabIndex={0} data-own-keys>
    <div hidden>
      <button>⋯ 3 unchanged lines</button>
    </div>
  </div>
);

/** Every kind of question the editor asks, and what Enter on each of its controls answers. A control
 *  the dialog gains without a line here fails the tests below, until it says what it answers. */
const KINDS: { kind: string; open: (d: Dialogs) => Promise<unknown>; answers: Record<string, unknown> }[] = [
  { kind: "confirm", open: (d) => d.confirm('Delete the saved flow "hello"?', "Delete"), answers: { Cancel: false, Delete: true } },
  {
    kind: "confirm with a review",
    open: (d) => d.confirm('Deploy "hello"?', "Approve and deploy", review),
    answers: { "what changes": UNANSWERED, Cancel: false, "Approve and deploy": true },
  },
  { kind: "prompt", open: (d) => d.prompt("Name this flow:", "  hello ", "Name it"), answers: { field: "hello", Cancel: null, "Name it": "hello" } },
];

describe("dialog keys", () => {
  it("answers yes with y and no with n, and shows both as chips outside the buttons' names", async () => {
    const d = mount();
    const { answer: yes } = await ask(() => d.confirm("Deploy it?", "Approve and deploy"));
    expect(buttons().map(nameOf)).toEqual(["Cancel", "Approve and deploy"]);
    expect(buttons().map((b) => b.querySelector("kbd")?.textContent)).toEqual(["n", "y"]);
    await press("y");
    await expect(yes).resolves.toBe(true);
    const { answer: no } = await ask(() => d.confirm("Remove it?", "Remove"));
    await press("n");
    await expect(no).resolves.toBe(false);
  });

  it("leaves Ctrl+N and the like to the browser", async () => {
    const d = mount();
    const { answer } = await ask(() => d.confirm("Discard?", "Discard"));
    await press("n", { ctrlKey: true });
    expect(dialog()).not.toBeNull();
    await press("Escape");
    await expect(answer).resolves.toBe(false);
  });

  it("keeps the keys that answer it, and Tab, from the page beneath", async () => {
    const d = mount();
    const seen: string[] = [];
    const listen = (e: KeyboardEvent) => seen.push(e.key);
    window.addEventListener("keydown", listen);
    const { answer } = await ask(() => d.confirm("Update it?", "Update"));
    await press("y");
    await expect(answer).resolves.toBe(true);
    const { answer: again } = await ask(() => d.confirm("Update it?", "Update"));
    await press("Tab");
    await press("Enter");
    await expect(again).resolves.toBe(false);
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
    await press("n", {}, field);
    await press("y", {}, field);
    expect(typed).toBe(2);
    expect(dialog()).not.toBeNull();
    act(() => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(field, "  shout ");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await press("Enter", {}, field);
    await expect(name).resolves.toBe("shout");
  });

  it("is a modal dialog named by its question", async () => {
    const d = mount();
    await ask(() => d.confirm('Delete the saved flow "hello"?', "Delete"));
    const box = dialog()!;
    expect(box.getAttribute("aria-modal")).toBe("true");
    expect(document.getElementById(box.getAttribute("aria-labelledby") ?? "")?.textContent).toBe('Delete the saved flow "hello"?');
  });
});

describe("the focused control answers", () => {
  // Guards: Enter answers as a click on the control that has the focus would, for every kind of
  // question and every control in it, reached by Tab as a keyboard user reaches it. Needed because
  // Enter used to answer yes wherever the focus was: tabbing to Cancel on "Delete the saved flow?"
  // or a deploy's consent and pressing Enter deleted, or approved. If it fails, some control in a
  // dialog answers other than it says (the message names which), and a keyboard or screen-reader
  // user can say no and be taken for yes; or the dialog gained a control the table doesn't name.
  it.each(KINDS)("$kind: Enter answers with each control it is pressed on", async ({ open, answers }) => {
    const d = mount();
    const { answer: first } = await ask(() => open(d));
    const controls = (await lap(dialog)).map(called);
    await press("Escape");
    await first;
    expect([...controls].sort()).toEqual(Object.keys(answers).sort());
    for (const [i, control] of controls.entries()) {
      let answered: unknown = UNANSWERED;
      await ask(() => open(d).then((a) => (answered = a)));
      for (let k = 0; k < i; k++) await press("Tab");
      expect(called(document.activeElement)).toBe(control);
      await press("Enter");
      expect(answered, `Enter on ${control}`).toEqual(answers[control]);
      if (answered === UNANSWERED) await press("Escape");
      expect(dialog(), `still open after Enter on ${control}`).toBeNull();
    }
  });

  // Guards: the Enter that answers one question doesn't also answer the question its answer opens,
  // whichever control it was pressed on, and that question starts on its own button, as any does.
  // Needed because a browser presses the focused button as Enter's own default, after the page has
  // handled the key: by then the next dialog is open and its button has the focus, so a prompt
  // answered with Enter would approve the confirm after it unread (AuthGate asks two prompts then a
  // confirm; a deploy asks after a save). And the two can come in one render, with no closed
  // dialog between: the next one must still take the focus. If it fails, a question can be
  // answered before it was shown, or one opens with the focus left on the last one's Cancel.
  it.each(KINDS)("$kind: the Enter that answers it doesn't answer the question it opens", async ({ open, answers }) => {
    const d = mount();
    const controls = Object.keys(answers).filter((c) => answers[c] !== UNANSWERED);
    for (const control of controls) {
      let second: unknown = UNANSWERED;
      await ask(() => open(d).then(() => d.confirm("Deploy it now?", "Deploy").then((a) => (second = a))));
      for (let k = 0; k < 10 && called(document.activeElement) !== control; k++) await press("Tab");
      expect(called(document.activeElement)).toBe(control);
      await press("Enter");
      expect(dialog()?.textContent, `after Enter on ${control}`).toContain("Deploy it now?");
      expect(second, `after Enter on ${control}`).toBe(UNANSWERED);
      expect(called(document.activeElement), `the focus after Enter on ${control}`).toBe("Deploy");
      await press("Escape");
      expect(second).toBe(false);
    }
  });
});
