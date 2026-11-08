// The address names every place, reads back as the same place, and says what is above it.

import { describe, expect, it } from "vitest";
import { crumbs, flowAddressable, format, labelOf, parentOf, parse, type Route } from "./routes";

/** Every place the console's design walks through, by its address. */
const PLACES = [
  "overview",
  "approvals",
  "approvals.runs",
  "approvals.refunds.v4",
  "flows",
  "refunds",
  "refunds.runs",
  "refunds.run-4812",
  "refunds.history",
  "refunds.state",
  "refunds.settings",
  "editor",
  "editor.home",
  "editor.refunds",
  "editor.refunds.view",
  "editor.refunds.view.run-12",
  "library",
  "library.tutorials",
  "library.market",
  "resources",
  "resources.folders",
  "people",
  "people.tokens",
  "policy",
  "start",
  "docs",
  "docs.language/tour.md",
  "docs.language/reference.md@forms-and-flows",
  "signin",
  "admin",
  "admin.people",
];

describe("routes", () => {
  it("reads every place back from the address it writes", () => {
    for (const h of PLACES) {
      const r = parse(`#${h}`);
      expect(r.kind, h).not.toBe("notfound");
      expect(format(r), h).toBe(`#${h}`);
      expect(parse(format(r)), h).toEqual(r);
    }
  });

  it("starts at the overview, and takes the hash with or without its #", () => {
    expect(parse("")).toEqual({ kind: "overview" });
    expect(parse("#")).toEqual({ kind: "overview" });
    expect(parse("refunds.runs")).toEqual(parse("#refunds.runs"));
  });

  it("reads a flow, its tabs, a run, and the editor's modes", () => {
    expect(parse("#refunds")).toEqual({ kind: "flow", slug: "refunds", tab: "status" });
    expect(parse("#refunds.history")).toEqual({ kind: "flow", slug: "refunds", tab: "history" });
    expect(parse("#refunds.run-7")).toEqual({ kind: "run", slug: "refunds", seq: 7 });
    expect(parse("#approvals.refunds.v4")).toEqual({ kind: "review", slug: "refunds", version: 4 });
    expect(parse("#editor")).toEqual({ kind: "editor", slug: null, view: false, seq: null });
    expect(parse("#editor.hello-api")).toEqual({ kind: "editor", slug: "hello-api", view: false, seq: null });
    expect(parse("#editor.hello-api.view.run-3")).toEqual({ kind: "editor", slug: "hello-api", view: true, seq: 3 });
    expect(parse("#library.samples")).toEqual({ kind: "library", tab: "samples" });
  });

  it("keeps a manual page's dots and slashes, and its heading", () => {
    expect(parse("#docs.language/tour.md")).toEqual({ kind: "docs", page: "language/tour.md", anchor: null });
    const r: Route = { kind: "docs", page: "guides/a b.md", anchor: "x@y" };
    expect(parse(format(r))).toEqual(r);
  });

  it("says what it doesn't know as not found", () => {
    for (const h of ["refunds.nope", "refunds.run-x", "editor.refunds.edit", "approvals.refunds", "approvals.refunds.4", "flows.extra", "bad name", "library.nope", "editor.a.view.run-1.more"]) {
      expect(parse(`#${h}`), h).toEqual({ kind: "notfound", hash: h });
    }
  });

  it("goes up from a run to its flow's runs, the flow, Flows and the overview", () => {
    const up = (h: string): string[] => {
      const chain: string[] = [];
      for (let r: Route | null = parse(h); r; r = parentOf(r)) chain.push(format(r));
      return chain;
    };
    expect(up("#refunds.run-4812")).toEqual(["#refunds.run-4812", "#refunds.runs", "#refunds", "#flows", "#overview"]);
    expect(up("#editor.refunds.view.run-3")).toEqual(["#editor.refunds.view.run-3", "#editor.refunds.view", "#editor.refunds", "#editor", "#overview"]);
    expect(up("#approvals.refunds.v4")).toEqual(["#approvals.refunds.v4", "#approvals", "#overview"]);
    expect(up("#docs.language/tour.md")).toEqual(["#docs.language/tour.md", "#docs", "#overview"]);
    expect(up("#people.tokens")).toEqual(["#people.tokens", "#people", "#overview"]);
  });

  it("says the crumbs from the overview down, each by its name", () => {
    const said = (h: string) => crumbs(parse(h)).map(labelOf);
    expect(said("#refunds.run-4812")).toEqual(["Overview", "Flows", "refunds", "runs", "run 4812"]);
    expect(said("#editor.refunds.view")).toEqual(["Overview", "Editor", "refunds", "deployed"]);
    expect(said("#docs.language/tour.md")).toEqual(["Overview", "Docs", "tour"]);
    expect(said("#overview")).toEqual(["Overview"]);
  });

  it("can't address a flow named like a page by its name alone", () => {
    expect(flowAddressable("refunds")).toBe(true);
    expect(flowAddressable("flows")).toBe(false);
    expect(flowAddressable("a.b")).toBe(false);
  });
});
