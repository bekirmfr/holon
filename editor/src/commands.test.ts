// The command palette's matching, a run found by its number, and `:` commands (commands.ts).

import { describe, expect, it } from "vitest";
import { matches, parseColon, ranked, runNumber, score, type Command } from "./commands";

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

describe("a run by its number", () => {
  it("reads 4812, #4812 and run 4812, and nothing else", () => {
    expect(runNumber("4812")).toBe(4812);
    expect(runNumber("#4812")).toBe(4812);
    expect(runNumber(" run 4812 ")).toBe(4812);
    expect(runNumber("Run #12")).toBe(12);
    expect(runNumber("runs")).toBeNull();
    expect(runNumber("4812 refunds")).toBeNull();
    expect(runNumber("")).toBeNull();
  });

  it("puts that run of each deployed flow first, then what else matches, once each", () => {
    const entry = (label: string, kind = "flow"): Command => ({ label, kind, run: () => undefined });
    const commands = [entry("Flows", "page"), entry("refunds runs"), entry("run #12 of refunds", "run"), entry("Theme: auto", "look")];
    const byNumber = (seq: number) => ["refunds", "hello-api"].map((slug) => entry(`run #${seq} of ${slug}`, "run"));
    expect(matches(commands, "#12", byNumber).map((c) => c.label)).toEqual(["run #12 of refunds", "run #12 of hello-api"]);
    // The newest runs' entry for the same run isn't shown twice.
    expect(matches(commands, "run 12", byNumber).map((c) => c.label)).toEqual(["run #12 of refunds", "run #12 of hello-api"]);
    expect(matches(commands, "4812", byNumber).map((c) => c.label)).toEqual(["run #4812 of refunds", "run #4812 of hello-api"]);
    // No number: as ranked.
    expect(matches(commands, "ref", byNumber)).toEqual(ranked(commands, "ref"));
  });
});

describe("parseColon", () => {
  it("reads a line, a command and a theme", () => {
    expect(parseColon(":check")).toEqual({ kind: "check" });
    expect(parseColon(":call")).toEqual({ kind: "call" });
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
