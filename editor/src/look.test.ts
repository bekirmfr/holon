// How the editor looks (look.ts): the interface set on <html>, kept in this browser, and the
// Terminal interface dark whatever the theme says.

import { afterEach, describe, expect, it } from "vitest";
import { applyTheme, applyUi, isDark, storedUi } from "./look";

afterEach(() => {
  applyTheme("auto");
  applyUi("clean");
});

describe("the interface", () => {
  it("is set on <html> as data-ui, and kept in this browser", () => {
    applyUi("terminal");
    expect(document.documentElement.getAttribute("data-ui")).toBe("b");
    expect(storedUi()).toBe("terminal");
    applyUi("clean");
    expect(document.documentElement.hasAttribute("data-ui")).toBe(false);
    expect(storedUi()).toBe("clean");
  });

  it("is dark in Terminal, even with the light theme chosen", () => {
    applyTheme("light");
    expect(isDark()).toBe(false);
    applyUi("terminal");
    expect(isDark()).toBe(true);
  });
});
