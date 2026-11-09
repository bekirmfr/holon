// The command palette's matching and `:` commands (commands.ts).

import { describe, expect, it } from "vitest";
import { parseColon, ranked, score } from "./commands";

describe("score", () => {
  it("ranks the label's start, then a word's start, then inside a word, then letters in order", () => {
    expect(score("Deploy bump", "dep")).toBe(0);
    expect(score("bump history", "his")).toBe(1);
    expect(score("bumphistory", "his")).toBe(3);
    expect(score("bump history", "bh")).toBe(8);
    expect(score("bump history", "x")).toBeNull();
    expect(score("bump history", "hb")).toBeNull();
    expect(score("anything", "")).toBe(0);
  });

  it("adds up its words", () => {
    expect(score("refunds runs", "ref runs")).toBe(1);
    expect(score("refunds runs", "ref zz")).toBeNull();
  });
});

describe("ranked", () => {
  it("puts the best first, keeping the order of ties", () => {
    const items = [{ label: "Edit refunds" }, { label: "refunds" }, { label: "refunds runs" }, { label: "Policy" }];
    expect(ranked(items, "ref").map((x) => x.label)).toEqual(["refunds", "refunds runs", "Edit refunds"]);
    expect(ranked(items, "").length).toBe(4);
  });
});

describe("parseColon", () => {
  it("reads a line, a command and a theme", () => {
    expect(parseColon(":12")).toEqual({ kind: "line", line: 12 });
    expect(parseColon("deploy")).toEqual({ kind: "deploy" });
    expect(parseColon(":w")).toEqual({ kind: "save" });
    expect(parseColon(":theme light")).toEqual({ kind: "theme", theme: "light" });
    expect(parseColon(":Theme DARK")).toEqual({ kind: "theme", theme: "dark" });
  });

  it("says why it reads none", () => {
    expect(parseColon(":0")).toEqual({ error: "lines start at 1" });
    expect(parseColon(":theme blue")).toEqual({ error: "a theme is auto, light or dark" });
    expect(parseColon(":")).toEqual({ error: "type a command after the colon" });
    expect(parseColon(":frob")).toEqual({ error: "there is no command :frob" });
  });
});
