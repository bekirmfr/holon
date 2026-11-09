import { describe, expect, it } from "vitest";
import { readPrefs, withPane } from "./home";

describe("a person's prefs", () => {
  it("read back from the server drop what isn't understood", () => {
    expect(readPrefs(null)).toEqual({});
    expect(readPrefs({ welcome: false, notes: "ship it", home: [{ id: "a", kind: "notes" }], extra: 1 })).toEqual({ welcome: false, notes: "ship it" });
    expect(readPrefs({ notes: 3 })).toEqual({});
    expect(readPrefs({ hidden: ["bottom", "palette", 3, "canvas"] })).toEqual({ hidden: ["canvas", "bottom"] });
    expect(readPrefs({ theme: "dark", ui: "terminal" })).toEqual({ theme: "dark", ui: "terminal" });
    expect(readPrefs({ ui: "b" })).toEqual({});
  });

  it("remember which of the editor's panes are hidden", () => {
    expect(withPane(undefined, "inspector", false)).toEqual(["inspector"]);
    expect(withPane(["inspector", "values"], "inspector", true)).toEqual(["values"]);
    expect(withPane(["values"], "values", false)).toEqual(["values"]);
  });
});
