// A question in the page answers to keys as well as clicks: Enter answers with what has the focus,
// Esc cancels, and a yes/no question takes y and n, shown as chips that stay out of the buttons'
// names. Keys that answer it don't reach the page beneath; typing in a prompt still reaches its
// field. A choice takes each answer's letter, and a notice has one button, and its letter. (Tab
// staying in it is every modal's: modals.test.tsx.) It takes no answer but no until it has been
// shown a moment with no key pressed: the last describe.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import ts from "typescript";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ARM_MS, DialogProvider, TOO_SOON, useDialog, WAIT_SAID } from "./Dialog";
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

/** The dialogs, over a page with a control of its own: where a Tab that left the dialog would go.
 *  Its questions take an answer at once, unless `armMs` says how long they wait first. */
function mount(armMs = 0): Dialogs {
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
      <DialogProvider armMs={armMs}>
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
  {
    kind: "choice",
    open: (d) => d.choose("Close it?", ["Keep it open", "Discard", "Save and close"]),
    answers: { "Keep it open": 0, Discard: 1, "Save and close": 2 },
  },
  { kind: "notice", open: (d) => d.notice("Your password is changed."), answers: { OK: undefined } },
  { kind: "notice with a review", open: (d) => d.notice("The chain of shop's runs", "Close", review), answers: { "what changes": UNANSWERED, Close: undefined } },
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

  it("holds a required answer until something is typed, saying why, and still cancels", async () => {
    const d = mount();
    const field = () => document.querySelector<HTMLInputElement>("[aria-modal=true] input")!;
    const type = (value: string) =>
      act(() => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field(), value);
        field().dispatchEvent(new Event("input", { bubbles: true }));
      });
    const decline = () => buttons().find((b) => nameOf(b) === "Decline")!;
    const { answer: why } = await ask(() => d.prompt("Decline it?", "", "Decline", { required: "A decline says why: type a reason." }));
    expect([decline().disabled, decline().title]).toEqual([true, "A decline says why: type a reason."]);
    expect(dialog()!.textContent).toContain("A decline says why: type a reason.");
    await press("Enter", {}, field());
    type("   ");
    await press("Enter", {}, field());
    expect(dialog()).not.toBeNull();
    type(" not this vendor ");
    expect(decline().disabled).toBe(false);
    expect(dialog()!.textContent).not.toContain("A decline says why");
    await press("Enter", {}, field());
    await expect(why).resolves.toBe("not this vendor");
    const { answer: none } = await ask(() => d.prompt("Decline it?", "", "Decline", { required: "A decline says why: type a reason." }));
    await press("Escape");
    await expect(none).resolves.toBeNull();
    // An optional answer may be empty; by default an empty one answers as Cancel does.
    const { answer: optional } = await ask(() => d.prompt("Approve it?", "", "Approve", "optional"));
    await press("Enter", {}, field());
    await expect(optional).resolves.toBe("");
    const { answer: needed } = await ask(() => d.prompt("Name it:"));
    await press("Enter", {}, field());
    await expect(needed).resolves.toBeNull();
  });

  it("gives each of a choice's answers its letter, the last answered by Enter too, Esc none", async () => {
    const d = mount();
    const three = ["Keep it open", "Discard", "Save and close"];
    const { answer: kept } = await ask(() => d.choose("Close it?", three));
    expect(buttons().map((b) => b.querySelector("kbd")?.textContent)).toEqual(["k", "d", "s"]);
    await press("k");
    await expect(kept).resolves.toBe(0);
    const { answer: saved } = await ask(() => d.choose("Close it?", three));
    await press("Enter");
    await expect(saved).resolves.toBe(2);
    const { answer: none } = await ask(() => d.choose("Close it?", three));
    await press("Escape");
    await expect(none).resolves.toBeNull();
  });

  it("shows a notice with its one button, closed by its letter, Enter or Esc", async () => {
    const d = mount();
    const { answer: lettered } = await ask(() => d.notice("The chain of shop's runs", "Close"));
    expect(buttons().map(nameOf)).toEqual(["Close"]);
    expect(buttons().map((b) => b.querySelector("kbd")?.textContent)).toEqual(["c"]);
    await press("c");
    await expect(lettered).resolves.toBeUndefined();
    expect(dialog()).toBeNull();
    for (const key of ["Enter", "Escape"]) {
      const { answer } = await ask(() => d.notice("Your password is changed."));
      expect(buttons().map(nameOf)).toEqual(["OK"]);
      await press(key);
      await expect(answer).resolves.toBeUndefined();
    }
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

describe("a prompt that offers an empty answer", () => {
  // Guards: a prompt whose question offers empty ("empty for none", "empty: wherever…") takes an
  // empty line as its answer and Cancel as none: every such prompt in the console is asked
  // `optional`. Needed because a prompt left empty answered null, as Cancel does, so "empty for
  // none" did nothing: an email, a marketplace's address or where a server's secret may go
  // couldn't be cleared. If it fails, the prompt it names drops the empty answer it offers.
  it("takes empty as its answer, and Cancel as none", async () => {
    const d = mount();
    for (const [press, answer] of [["OK", ""], ["Cancel", null]] as const) {
      const { answer: got } = await ask(() => d.prompt("An email (empty for none):", "ada@example.com", undefined, "optional"));
      const field = dialog()!.querySelector("input")!;
      act(() => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, "");
        field.dispatchEvent(new Event("input", { bubbles: true }));
      });
      act(() => buttons().find((b) => nameOf(b) === press)!.click());
      expect(await got, press).toBe(answer);
    }
  });

  it("is asked so, wherever the console's text offers it", () => {
    const dir = join(process.cwd(), "src");
    const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f));
    const offering: string[] = [];
    const dropped: string[] = [];
    for (const file of files) {
      const source = ts.createSourceFile(file, readFileSync(join(dir, file), "utf8"), ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
      const visit = (node: ts.Node) => {
        if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === "prompt" && node.arguments.length > 0) {
          const question = node.arguments[0].getText();
          if (/\bempty\b/i.test(question)) {
            const where = `${file.replace(/\\/g, "/")}:${source.getLineAndCharacterOfPosition(node.getStart()).line + 1}`;
            offering.push(where);
            if (node.arguments[3]?.getText() !== '"optional"') dropped.push(where);
          }
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
    expect(offering.length, "the console's prompts that offer empty are found").toBeGreaterThanOrEqual(3);
    expect(dropped).toEqual([]);
  });
});

describe("a question for a secret", () => {
  // Guards: a secret typed into the console (a password, a key) is never shown. It is asked with
  // `secret`, whose field hides it (a password field, filled by a password manager as its
  // `autoComplete` says) and which answers it as typed; and every question in the console for a
  // password or a key is asked so. Needed because changing a password asked the current one and
  // the new one in a line of plain text, for anyone looking at the screen to read, and the
  // marketplace's publish key the same. If it fails, the question it names shows what is typed
  // into it: ask it with `secret`.
  const type = (field: HTMLInputElement, text: string) =>
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, text);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });

  it("hides what is typed, and answers it as typed", async () => {
    const d = mount();
    for (const fill of ["current-password", "new-password", "off"] as const) {
      const typed = `  correct horse ${fill}  `;
      const { answer } = await ask(() => d.secret("Your current password:", fill));
      const field = dialog()!.querySelector("input")!;
      expect(field.type, fill).toBe("password");
      expect(field.getAttribute("autocomplete"), fill).toBe(fill);
      type(field, typed);
      expect(document.body.textContent, `${fill}: not in the page's text`).not.toContain("horse");
      act(() => buttons().find((b) => nameOf(b) === "OK")!.click());
      expect(await answer, `${fill}: its spaces its own`).toBe(typed);
    }
    // Left empty, or cancelled: none.
    for (const press of ["OK", "Cancel"]) {
      const { answer } = await ask(() => d.secret("A key:", "off"));
      act(() => buttons().find((b) => nameOf(b) === press)!.click());
      expect(await answer, press).toBeNull();
    }
    // A prompt after it is a line of text again, opened empty.
    const { answer } = await ask(() => d.prompt("A name:"));
    expect(dialog()!.querySelector("input")!.type).toBe("text");
    expect(dialog()!.querySelector("input")!.value).toBe("");
    act(() => buttons().find((b) => nameOf(b) === "Cancel")!.click());
    await answer;
  });

  it("is how every password and key is asked in the console", () => {
    const dir = join(process.cwd(), "src");
    const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f));
    const SECRET = /password|passphrase|\bkey\b/i;
    const asked: string[] = [];
    const shown: string[] = [];
    for (const file of files) {
      const source = ts.createSourceFile(file, readFileSync(join(dir, file), "utf8"), ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
      const visit = (node: ts.Node) => {
        if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.arguments.length > 0) {
          const where = `${file.replace(/\\/g, "/")}:${source.getLineAndCharacterOfPosition(node.getStart()).line + 1}`;
          const how = node.expression.name.text;
          if (how === "secret") asked.push(where);
          if (how === "prompt" && SECRET.test(node.arguments[0].getText())) shown.push(where);
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
    // A password changed (the current one, the new one twice) and the marketplace's key (twice).
    expect(asked.length, "the console's questions for a secret are found").toBeGreaterThanOrEqual(5);
    expect(shown).toEqual([]);
  });
});

describe("a question that opens while you type", () => {
  // Guards: a question takes no answer but no until it has been shown ARM_MS with no key pressed,
  // for every kind and every control that answers it; a key or a click on an answer while it waits
  // starts the wait again, a held key never answers, and the letters typed into a prompt's field
  // still reach it. Needed because a question can open a moment after the key that asked for it,
  // with the focus on its yes (a deploy's consent, read after Ctrl+D, which sits beside Ctrl+S):
  // the next Enter or y typed for the page answered it unread (R43). If it fails, a key or a click
  // meant for something else answers a question before it is read, or a no is held up.
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  });
  afterEach(() => {
    vi.useRealTimers();
  });
  /** A key pressed as the browser sends it, without `press`'s waits: the clock is the test's. */
  const key = (k: string, init: KeyboardEventInit = {}, at: Element = document.activeElement ?? document.body) => {
    const event = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
    act(() => {
      at.dispatchEvent(event);
    });
    return event;
  };
  const later = (ms: number) =>
    act(() => {
      vi.advanceTimersByTime(ms);
    });
  const flush = () => act(async () => {});
  const waiting = () => buttons().filter((b) => b.getAttribute("aria-disabled") === "true").map(nameOf);
  /** The control a table row names, in the open question. */
  const control = (name: string) => [...dialog()!.querySelectorAll("button, input, [data-own-keys]")].find((el) => called(el) === name) as HTMLElement;

  it.each(KINDS)("$kind: Enter and a click answer only once the wait is over; Cancel at once", async ({ open, answers }) => {
    const d = mount(ARM_MS);
    for (const [name, answer] of Object.entries(answers)) {
      if (answer === UNANSWERED) continue;
      let answered: unknown = UNANSWERED;
      await ask(() => open(d).then((a) => (answered = a)));
      control(name).focus();
      const enter = key("Enter");
      await flush();
      expect(enter.defaultPrevented, `Enter on ${name} is kept from the button`).toBe(true);
      if (name === "Cancel") {
        expect(answered, "Cancel answers at once").toEqual(answer);
        continue;
      }
      expect(answered, `Enter on ${name} at once`).toBe(UNANSWERED);
      expect(waiting().length, "its answers look disabled").toBeGreaterThan(0);
      if (control(name) instanceof HTMLButtonElement) {
        act(() => control(name).click());
        await flush();
        expect(answered, `a click on ${name} at once`).toBe(UNANSWERED);
      }
      later(ARM_MS);
      expect(waiting()).toEqual([]);
      control(name).focus();
      key("Enter");
      await flush();
      expect(answered, `Enter on ${name} once the wait is over`).toEqual(answer);
      expect(dialog()).toBeNull();
    }
  });

  it.each(KINDS)("$kind: a screen reader hears why its answers wait, and that one came too soon", async ({ open, answers }) => {
    const d = mount(ARM_MS);
    await ask(() => open(d));
    const status = () => dialog()!.querySelector('[role="status"]')!.textContent;
    const described = (el: Element) => (el.getAttribute("aria-describedby") ?? "").split(" ").map((id) => document.getElementById(id)?.textContent ?? "").join(" ");
    const answering = Object.entries(answers).filter(([name, a]) => a !== UNANSWERED && name !== "Cancel" && name !== "field");
    for (const [name] of answering) expect(described(control(name)), `${name}, dimmed, says why`).toBe(WAIT_SAID);
    expect(status(), "nothing said before an answer comes").toBe("");
    const [first] = answering[0];
    control(first).focus();
    key("Enter");
    await flush();
    expect(status(), "an answer too soon is said not taken").toBe(TOO_SOON);
    later(ARM_MS);
    for (const [name] of answering) expect(control(name).getAttribute("aria-describedby"), `${name}, taking answers, says nothing more`).toBeNull();
  });

  it("waits again after each key pressed while it waits, and takes no answer from a held key", async () => {
    const d = mount(ARM_MS);
    let answered: unknown = UNANSWERED;
    await ask(() => d.confirm('Deploy "shop"?', "Approve and deploy").then((a) => (answered = a)));
    control("Approve and deploy").focus();
    later(ARM_MS - 100);
    key("e");
    later(200);
    key("y");
    await flush();
    expect(answered, "a y typed 200 ms after another key").toBe(UNANSWERED);
    later(ARM_MS - 1);
    expect(waiting()).toEqual(["Approve and deploy"]);
    later(1);
    expect(waiting()).toEqual([]);
    key("y", { repeat: true });
    key("Enter", { repeat: true });
    await flush();
    expect(answered, "a held key").toBe(UNANSWERED);
    key("y");
    await flush();
    expect(answered).toBe(true);
  });

  it("takes no at once: n, Esc, Cancel, a click beside it", async () => {
    const d = mount(ARM_MS);
    const nos: [string, () => void][] = [
      ["n", () => key("n")],
      ["Esc", () => key("Escape")],
      ["Cancel", () => act(() => control("Cancel").click())],
      ["a click beside it", () => act(() => void dialog()!.parentElement!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })))],
    ];
    for (const [what, no] of nos) {
      let answered: unknown = UNANSWERED;
      await ask(() => d.confirm("Remove it?", "Remove").then((a) => (answered = a)));
      no();
      await flush();
      expect(answered, what).toBe(false);
    }
  });

  it("lets a prompt's field be typed into while it waits, the letters not starting it again", async () => {
    const d = mount(ARM_MS);
    let answered: unknown = UNANSWERED;
    await ask(() => d.prompt("Name this flow:", "", "Name it").then((a) => (answered = a)));
    const field = document.querySelector<HTMLInputElement>("[aria-modal=true] input")!;
    later(ARM_MS - 100);
    expect(key("s", {}, field).defaultPrevented, "the letter reaches the field").toBe(false);
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, "shout");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    later(100);
    key("Enter", {}, field);
    await flush();
    expect(answered).toBe("shout");
  });

  it("waits again for the question an answer opens", async () => {
    const d = mount(ARM_MS);
    let second: unknown = UNANSWERED;
    await ask(() => d.confirm("Save it?", "Save").then(() => d.confirm("Deploy it now?", "Deploy").then((a) => (second = a))));
    later(ARM_MS);
    key("y");
    await flush();
    expect(dialog()?.textContent).toContain("Deploy it now?");
    key("y");
    await flush();
    expect(second).toBe(UNANSWERED);
    later(ARM_MS);
    key("y");
    await flush();
    expect(second).toBe(true);
  });
});
