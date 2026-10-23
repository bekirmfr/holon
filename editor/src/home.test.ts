import { describe, expect, it } from "vitest";
import { addPane, changePane, defaultHome, movePane, readPrefs, removePane } from "./home";

describe("the home tab's panes", () => {
  it("are added with ids of their own, moved, widened and removed", () => {
    let panes = addPane(addPane(defaultHome(), "notes"), "notes");
    expect(panes.slice(-2).map((p) => p.id)).toEqual(["notes-1", "notes-2"]);
    panes = movePane(panes, "notes-2", -1);
    expect(panes.slice(-2).map((p) => p.id)).toEqual(["notes-2", "notes-1"]);
    expect(movePane(panes, panes[0].id, -1)).toBe(panes);
    panes = changePane(panes, "notes-1", { wide: true, note: "ship it" });
    expect(panes.find((p) => p.id === "notes-1")).toMatchObject({ wide: true, note: "ship it" });
    expect(removePane(panes, "notes-1").some((p) => p.id === "notes-1")).toBe(false);
  });

  it("read back from the server drop what isn't understood", () => {
    expect(readPrefs(null)).toEqual({});
    expect(readPrefs({ welcome: false, home: [{ id: "a", kind: "notes" }, { id: "b", kind: "nope" }, null], extra: 1 })).toEqual({
      welcome: false,
      home: [{ id: "a", kind: "notes", wide: false }],
    });
  });
});
