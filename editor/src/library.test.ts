// The Library's chips (library.ts): what a sample does, read from what deploying it asks for.

import { describe, expect, it } from "vitest";
import { capabilitiesOf, keywordsOf, kindOf, matches } from "./library";

describe("the library's chips", () => {
  it("read an item's kind", () => {
    expect(kindOf('DbRead("shop/orders")')).toBe("DbRead");
    expect(kindOf("Clock")).toBe("Clock");
    expect(kindOf("Limit(keep_runs=5)")).toBe("Limit");
  });

  it("say what a flow does, in the chips' order, and nothing for answering requests", () => {
    expect(capabilitiesOf(["NetworkIngress"])).toEqual([]);
    expect(capabilitiesOf(['HttpRequest("httpbin.org")', 'DbWrite("bank/payouts")', 'Await("approved")', "NetworkIngress"])).toEqual(["http", "db", "wait"]);
    expect(capabilitiesOf(['CollectionRead("notes")', "Clock", 'SecretRead("k")'])).toEqual(["state", "secret", "clock"]);
  });

  it("show a sample with every chip picked, found by its words", () => {
    const sample = { name: "fetch-json", title: "Fetch JSON", description: "Calls an API and answers its JSON", shows: ["request", "json_parse"] };
    expect(matches(sample, ["http"], [], "")).toBe(true);
    expect(matches(sample, ["http"], ["http"], "")).toBe(true);
    expect(matches(sample, ["http"], ["http", "db"], "")).toBe(false);
    expect(matches(sample, ["http"], [], "json_PARSE")).toBe(true);
    expect(matches(sample, ["http"], [], "ledger")).toBe(false);
  });

  it("find a sample by keywords, every one somewhere in its words, in any order", () => {
    const sample = { name: "fetch-json", title: "Fetch JSON", description: "Calls an API and answers its JSON", shows: ["request", "json_parse"], tryIt: "curl http://127.0.0.1:8130/user" };
    expect(keywordsOf("  JSON,  api ")).toEqual(["json", "api"]);
    expect(matches(sample, ["http"], [], "api fetch")).toBe(true);
    expect(matches(sample, ["http"], [], "api ledger")).toBe(false);
    expect(matches(sample, ["http"], [], "call out"), "its chips").toBe(true);
    expect(matches(sample, ["http"], [], "curl 8130"), "how to try it").toBe(true);
    expect(matches(sample, ["http"], [], "httpbin"), "what it may do").toBe(false);
    expect(matches(sample, ["http"], [], "httpbin", ["call https://httpbin.org"])).toBe(true);
  });
});
