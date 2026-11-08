// The keys (keymap.ts, ui/Keys.tsx): how a key is named, when it is being typed rather than
// pressed, stepping through a run, and the bindings standing aside for fields and dialogs.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import { KEYMAP, stepOf } from "./keymap";
import { keyOf, typing, useKeys } from "./ui/Keys";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const key = (key: string, mods: Partial<Record<"ctrlKey" | "metaKey" | "altKey" | "shiftKey", boolean>> = {}) => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe("keyOf", () => {
  it("names a key as the bindings do", () => {
    expect(keyOf(key("s", { ctrlKey: true }))).toBe("ctrl+s");
    expect(keyOf(key("S", { ctrlKey: true, shiftKey: true }))).toBe("ctrl+s");
    expect(keyOf(key("s", { metaKey: true }))).toBe("ctrl+s");
    expect(keyOf(key("?", { shiftKey: true }))).toBe("?");
    expect(keyOf(key("N", { shiftKey: true }))).toBe("N");
    expect(keyOf(key("F6", { shiftKey: true }))).toBe("shift+F6");
    expect(keyOf(key("Escape"))).toBe("Escape");
    expect(keyOf(key(" "))).toBe("Space");
  });
});

describe("typing", () => {
  it("is a field, an editable element or the text pane", () => {
    const input = document.createElement("input");
    const cm = document.createElement("div");
    cm.className = "cm-editor";
    const line = document.createElement("div");
    cm.appendChild(line);
    const plain = document.createElement("div");
    expect(typing(input)).toBe(true);
    expect(typing(document.createElement("textarea"))).toBe(true);
    expect(typing(line)).toBe(true);
    expect(typing(plain)).toBe(false);
    expect(typing(null)).toBe(false);
  });
});

describe("stepOf", () => {
  it("steps through a run's nodes, wrapping through the whole run", () => {
    expect(stepOf(null, 3, 1)).toBe(0);
    expect(stepOf(0, 3, 1)).toBe(1);
    expect(stepOf(2, 3, 1)).toBe(null);
    expect(stepOf(null, 3, -1)).toBe(2);
    expect(stepOf(1, 3, -1)).toBe(0);
    expect(stepOf(0, 3, -1)).toBe(null);
  });
});

describe("KEYMAP", () => {
  it("lists each key once in a scope", () => {
    for (const scope of new Set(KEYMAP.map((b) => b.scope))) {
      const keys = KEYMAP.filter((b) => b.scope === scope).flatMap((b) => b.keys.filter((k) => k !== "…"));
      expect(new Set(keys).size).toBe(keys.length);
    }
  });
});

describe("useKeys", () => {
  let host: HTMLDivElement;
  afterEach(() => host?.remove());

  function mount(map: Parameters<typeof useKeys>[0], opts?: Parameters<typeof useKeys>[1]) {
    host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    function Probe() {
      useKeys(map, opts);
      return createElement("input", { "aria-label": "field" });
    }
    act(() => root.render(createElement(Probe)));
    return root;
  }
  const press = (target: EventTarget, k: string, init: KeyboardEventInit = {}) => {
    const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
    target.dispatchEvent(e);
    return e;
  };

  it("takes its keys, and leaves the rest", () => {
    const j = vi.fn();
    const root = mount({ j });
    expect(press(document.body, "j").defaultPrevented).toBe(true);
    expect(press(document.body, "x").defaultPrevented).toBe(false);
    expect(j).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it("stands aside while typing, and while a dialog is open, unless told not to", () => {
    const j = vi.fn();
    const save = vi.fn();
    const root = mount({ j, "ctrl+s": save }, { whileTyping: ["ctrl+s"] });
    const field = host.querySelector("input")!;
    press(field, "j");
    press(field, "s", { ctrlKey: true });
    expect(j).not.toHaveBeenCalled();
    expect(save).toHaveBeenCalledTimes(1);
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.appendChild(dialog);
    press(document.body, "j");
    expect(j).not.toHaveBeenCalled();
    dialog.remove();
    press(document.body, "j");
    expect(j).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it("lets a key go on when its binding didn't take it", () => {
    const root = mount({ Escape: () => false });
    expect(press(document.body, "Escape").defaultPrevented).toBe(false);
    act(() => root.unmount());
  });
});
