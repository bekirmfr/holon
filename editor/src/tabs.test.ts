import { describe, expect, it } from "vitest";
import { blankTab, nextAfterClose, openChoice, tabKind } from "./tabs";

describe("tabs", () => {
  it("open a flow that is open already by going to its tab, never over its changes", () => {
    const tabs = [{ name: "todo" }, { name: null }];
    expect(openChoice("todo", tabs)).toBe("switch");
    expect(openChoice("other", tabs)).toBe("load");
    expect(openChoice("todo", [])).toBe("load");
  });

  it("say what each flow is: deployed, saved, or a draft", () => {
    const deployed = (name: string) => name === "likes";
    expect(tabKind({ name: "likes", saved: true }, deployed)).toBe("deployed");
    expect(tabKind({ name: "likes", saved: false }, deployed), "a deployment without a saved flow").toBe("deployed");
    expect(tabKind({ name: "notes", saved: true }, deployed)).toBe("saved");
    expect(tabKind({ name: "notes", saved: false }, deployed)).toBe("draft");
    expect(tabKind({ name: null, saved: false }, deployed)).toBe("draft");
  });

  it("show the tab before a closed one, else the one after, else none", () => {
    expect(nextAfterClose([0, 1, 2], 1)).toBe(0);
    expect(nextAfterClose([0, 1, 2], 0)).toBe(1);
    expect(nextAfterClose([0, 1, 2], 2)).toBe(1);
    expect(nextAfterClose([4], 4)).toBeNull();
  });

  it("start a tab in edit mode, empty", () => {
    const t = blankTab("x");
    expect([t.viewing, t.nodes.length, t.saved, t.dirty]).toEqual([null, 0, false, false]);
  });
});
