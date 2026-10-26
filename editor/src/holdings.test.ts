import { describe, expect, it } from "vitest";
import { holdersOf, itemParts, kindsHeld, othersHeld, reaches, seenBy, usesText } from "./holdings";
import type { HeldGrant } from "./server";

const held: HeldGrant[] = [
  {
    slug: "orders-api",
    status: "running",
    attention: null,
    items: ['DbRead("shop/orders")', 'DbWrite("shop/orders")', 'DbRead("shop/refunds")', 'HttpRequest("api.stripe.com")', "NetworkIngress", "Limit(max_requests=80)"],
    revoked: ['DbWrite("shop/refunds")'],
  },
  { slug: "drop-box", status: "paused", attention: "it needs FileWrite(\"inbox\")", items: ['FileRead("inbox")', 'FileArrival("inbox")', 'SecretRead("key")'], revoked: ['FileWrite("inbox")'] },
  // Bound its own "shop": the server's isn't the one it reaches.
  { slug: "audit", status: "running", attention: null, items: ['DbRead("shop/orders")', 'DbRead("ledger/lines")', 'HttpRequest("api.stripe.com")'], revoked: [] },
];
const databases = [
  { name: "shop", deployment: null },
  { name: "shop", deployment: "audit" },
];
const folders = [{ name: "inbox", deployment: null }];

describe("who holds what", () => {
  it("reads an item's kind and argument, and what it reaches", () => {
    expect(itemParts('DbWrite("shop/orders")')).toEqual({ kind: "DbWrite", arg: "shop/orders" });
    expect(itemParts("Limit(max_requests=80)")).toEqual({ kind: "Limit", arg: null });
    expect(itemParts('SecretRead("a\\"b")')).toEqual({ kind: "SecretRead", arg: 'a"b' });
    expect(reaches('DbRead("shop/sales.orders")')).toEqual({ kind: "database", name: "shop", use: "read", table: "sales.orders" });
    expect(reaches('FileArrival("inbox")')).toEqual({ kind: "folder", name: "inbox", use: "arrival", table: null });
    expect(reaches('HttpRequest("a.com")')).toBeNull();
  });

  it("finds the binding a deployment sees: its own, else the server's", () => {
    expect(seenBy(databases, "shop", "audit")).toEqual({ name: "shop", deployment: "audit" });
    expect(seenBy(databases, "shop", "orders-api")).toEqual({ name: "shop", deployment: null });
    expect(seenBy(databases, "ledger", "audit")).toBeUndefined();
  });

  it("lists who reaches a binding, through the binding each sees, revoked items among them", () => {
    const server = holdersOf("database", databases[0], databases, held);
    expect(server.map((h) => h.slug)).toEqual(["orders-api"]);
    expect(usesText(server[0].uses)).toEqual([
      { text: "reads orders, refunds", revoked: false },
      { text: "writes orders", revoked: false },
      { text: "writes refunds", revoked: true },
    ]);
    const own = holdersOf("database", databases[1], databases, held);
    expect(own.map((h) => [h.slug, usesText(h.uses)[0].text])).toEqual([["audit", "reads orders"]]);
    const inbox = holdersOf("folder", folders[0], folders, held);
    expect(inbox.map((h) => [h.slug, h.status, usesText(h.uses).map((u) => u.text)])).toEqual([["drop-box", "paused", ["reads", "starts on new files", "writes"]]]);
  });

  it("puts every other item by item, a name no binding answers among them, limits left out", () => {
    const others = othersHeld(held, folders, databases);
    expect(others.map((o) => [o.item, o.kind, o.holders.map((h) => h.slug)])).toEqual([
      ['DbRead("ledger/lines")', "databases", ["audit"]],
      ['HttpRequest("api.stripe.com")', "HttpRequest", ["orders-api", "audit"]],
      ["NetworkIngress", "NetworkIngress", ["orders-api"]],
      ['SecretRead("key")', "SecretRead", ["drop-box"]],
    ]);
    expect(kindsHeld(held)).toEqual(["databases", "folders", "HttpRequest", "NetworkIngress", "SecretRead"]);
  });
});
