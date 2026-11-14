// The keys (keymap.ts, ui/Keys.tsx): how a key is named, when it is being typed rather than
// pressed, stepping through a run, the bindings standing aside for fields and dialogs, a page's
// key before an anywhere key, a list moved through from the keys, and KEYMAP held to the code:
// every key it lists is bound somewhere, and every key bound is listed.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { KEYMAP, lettersOf, stepOf, type KeyScope } from "./keymap";
import { keyOf, pick, shownKeys, typing, useKeys, useListKeys } from "./ui/Keys";

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
      const keys = KEYMAP.filter((b) => b.scope === scope).flatMap((b) => b.keys);
      expect(new Set(keys).size).toBe(keys.length);
    }
  });

  it("shows a run of digits as its ends", () => {
    expect(shownKeys(["1", "2", "3", "4", "5", "6", "7", "8"])).toEqual(["1", "…", "8"]);
    expect(shownKeys([",", "."])).toEqual([",", "."]);
    expect(shownKeys(["1", "3", "4"])).toEqual(["1", "3", "4"]);
  });

  // Every place that binds keys names its scope: useKeys(scope, {…}) or keysFor(scope, {…}).
  // The types already hold each key bound to KEYMAP; read from the code, the keys bound in each
  // scope are exactly the keys KEYMAP lists there, so the `?` dialog says all of them and only
  // them. A dialog's other buttons (lettersOf) have letters of their own, said by its y and n row.
  it("lists exactly the keys the code binds, scope by scope", () => {
    const bound = boundKeys();
    const scopes = new Set<KeyScope>([...KEYMAP.map((b) => b.scope), ...(bound.keys() as Iterable<KeyScope>)]);
    for (const scope of scopes) {
      const listed = new Set(KEYMAP.filter((b) => b.scope === scope).flatMap((b) => b.keys));
      expect({ scope, keys: [...(bound.get(scope) ?? [])].sort() }).toEqual({ scope, keys: [...listed].sort() });
    }
  });
});

/** The keys each scope binds, read from the editor's code: the object given to every useKeys and
 *  keysFor call, through spreads and conditionals, never into the functions it holds. */
function boundKeys(): Map<string, Set<string>> {
  const dir = join(process.cwd(), "src");
  const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f));
  const bound = new Map<string, Set<string>>();
  const keysOf = (node: ts.Node): string[] => {
    if (ts.isParenthesizedExpression(node)) return keysOf(node.expression);
    if (ts.isConditionalExpression(node)) return [...keysOf(node.whenTrue), ...keysOf(node.whenFalse)];
    if (!ts.isObjectLiteralExpression(node)) throw new Error(`a binding's keys that can't be read: ${node.getText()}`);
    return node.properties.flatMap((p) => {
      if (ts.isSpreadAssignment(p)) return keysOf(p.expression);
      const name = p.name;
      if (name && (ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name))) return [name.text];
      throw new Error(`a key that can't be read: ${p.getText()}`);
    });
  };
  for (const file of files) {
    const text = readFileSync(join(dir, file), "utf8");
    if (!/\b(useKeys|keysFor)\(/.test(text)) continue;
    const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
    const visit = (node: ts.Node) => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && ["useKeys", "keysFor"].includes(node.expression.text)) {
        const [scope, map] = node.arguments;
        // The definitions themselves name it with a variable: only calls with a scope named.
        if (scope && ts.isStringLiteral(scope) && map) {
          const set = bound.get(scope.text) ?? new Set<string>();
          for (const k of keysOf(map)) set.add(k);
          bound.set(scope.text, set);
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return bound;
}

describe("lettersOf", () => {
  it("gives each button the first letter of its label not already taken", () => {
    expect(lettersOf(["Change password", "Getting started", "Sign out"], ["s", "d"])).toEqual(["c", "g", "o"]);
    expect(lettersOf(["Keep it open", "Discard", "Save and close"])).toEqual(["k", "d", "s"]);
    expect(lettersOf(["Deploy", "Delete"])).toEqual(["d", "e"]);
    expect(lettersOf(["ok", "ok"], ["o", "k"])).toEqual(["", ""]);
  });
});

describe("useKeys", () => {
  let host: HTMLDivElement;
  afterEach(() => host?.remove());

  function mount(bind: () => void) {
    host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    function Probe() {
      bind();
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
    const root = mount(() => useKeys("list", { j }));
    expect(press(document.body, "j").defaultPrevented).toBe(true);
    expect(press(document.body, "x").defaultPrevented).toBe(false);
    expect(j).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it("stands aside while typing, and while a dialog is open, unless told not to", () => {
    const j = vi.fn();
    const save = vi.fn();
    const root = mount(() => {
      useKeys("list", { j });
      useKeys("editor", { "ctrl+s": save }, { whileTyping: ["ctrl+s"] });
    });
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
    const root = mount(() => useKeys("anywhere", { Escape: () => false }));
    expect(press(document.body, "Escape").defaultPrevented).toBe(false);
    act(() => root.unmount());
  });

  it("gives a page's key before an anywhere key of the same name, bound first or not", () => {
    const docs = vi.fn();
    const diff = vi.fn();
    const root = mount(() => {
      useKeys("anywhere", { d: docs });
      useKeys("review", { d: diff });
    });
    press(document.body, "d");
    expect(diff).toHaveBeenCalledTimes(1);
    expect(docs).not.toHaveBeenCalled();
    act(() => root.unmount());
    // Not taken by the page (nothing to do there), it is the anywhere key's.
    const again = mount(() => {
      useKeys("anywhere", { d: docs });
      useKeys("review", { d: () => false });
    });
    press(document.body, "d");
    expect(docs).toHaveBeenCalledTimes(1);
    act(() => again.unmount());
  });

  it("moves through a list with j and k, opens the row with Enter, and keeps the row per list", () => {
    const open = vi.fn();
    let at: string | null = null;
    const list = () => {
      at = useListKeys("test-flows", ["a", "b", "c"], open).at;
    };
    const root = mount(list);
    expect(at).toBeNull();
    press(document.body, "Enter");
    expect(open).not.toHaveBeenCalled();
    act(() => void press(document.body, "j"));
    act(() => void press(document.body, "j"));
    expect(at).toBe("b");
    act(() => void press(document.body, "k"));
    act(() => void press(document.body, "k"));
    expect(at).toBe("a");
    act(() => void press(document.body, "j"));
    press(document.body, "Enter");
    expect(open).toHaveBeenCalledWith("b");
    act(() => root.unmount());
    // Back on the page, the same row; another list's row picked from elsewhere (the palette).
    const back = mount(list);
    expect(at).toBe("b");
    act(() => back.unmount());
    pick("test-runs", "z");
    let other: string | null = null;
    const third = mount(() => {
      other = useListKeys("test-runs", ["y", "z"], open).at;
    });
    expect(other).toBe("z");
    act(() => third.unmount());
  });
});
