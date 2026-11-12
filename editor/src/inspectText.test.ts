// What the Inspector says at the Statements level (web-console-L4.md §5.3, §13): the cursor on a
// name the flow imports is that import, on a name the language knows is that name, on a statement
// (its own name, where picking its row puts the cursor, or a name it binds) is the statement; what
// the flow may do through an import is each item naming it.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { holonGraph, inspectAt } from "./holonText";
import { importsOf, textCase, through, wordAt } from "./inspectText";
import { samples } from "./samples";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

const sample = (name: string) => samples().find((s) => s.name === name)!.text;
/** The case for the cursor put `by` characters into the first `needle` of `text`. */
const at = (text: string, needle: string, by = 1) => {
  const offset = text.indexOf(needle) + by;
  expect(offset, needle).toBeGreaterThan(by - 1);
  return textCase(text, offset, holonGraph(text), inspectAt(text, offset));
};

describe("the imports", () => {
  it("are each name a flow imports from outside itself, not the language's", () => {
    const text = [
      'import { flow, route } from "holon"',
      'import { now } from "holon:clock"',
      'import shop from "database:shop"',
      'import state from "state"',
      'import key from "secret:key" with { optional: "true" }',
      'import { charge, refund as back } from "flow:billing"',
      'import hooks from "host:hooks.slack.com"',
    ].join("\n");
    expect(importsOf(text)).toEqual([
      { name: "shop", kind: "database", target: "shop", line: 3, optional: false },
      { name: "state", kind: "state", target: "state", line: 4, optional: false },
      { name: "key", kind: "secret", target: "key", line: 5, optional: true },
      { name: "charge", kind: "flow", target: "billing", line: 6, optional: false },
      { name: "back", kind: "flow", target: "billing", line: 6, optional: false },
      { name: "hooks", kind: "host", target: "hooks.slack.com", line: 7, optional: false },
    ]);
  });

  it("are done through by the items that name them", () => {
    const shop = importsOf('import shop from "database:shop"')[0];
    expect(through('DbWrite("shop/orders")', shop)).toBe(true);
    expect(through('DbRead("shopping/orders")', shop)).toBe(false);
    const hooks = importsOf('import hooks from "host:hooks.slack.com"')[0];
    expect(through('HttpRequest("hooks.slack.com")', hooks)).toBe(true);
    const state = importsOf('import state from "state"')[0];
    expect(through('StateWrite("greeted")', state)).toBe(true);
    expect(through('CollectionWrite("greeted")', state)).toBe(false);
  });

  it("are words, not a member of one or a number", () => {
    expect(wordAt("shop.one(x)", 2)).toEqual({ word: "shop", from: 0, member: false });
    expect(wordAt("shop.one(x)", 6)).toEqual({ word: "one", from: 5, member: true });
    expect(wordAt("a + 12", 5)).toBeNull();
  });
});

describe("the Inspector's case at the cursor", () => {
  const orders = sample("orders-api");

  it("is the import, on an imported name: where it is imported, and where it is used", () => {
    const declared = at(orders, "import shop", 8);
    expect(declared?.case).toBe("import");
    expect(declared?.case === "import" && declared.imported).toMatchObject({ name: "shop", kind: "database", target: "shop" });
    expect(at(orders, "shop.exec", 1)?.case).toBe("import");
  });

  it("is the name, on one the language knows", () => {
    const route = at(orders, "route.post", 2);
    expect(route?.case).toBe("word");
    expect(route?.case === "word" && route.word.word).toBe("route");
    // A method of an import is the language's too.
    const exec = at(orders, "shop.exec", 6);
    expect(exec?.case === "word" && exec.word.word).toBe("exec");
  });

  it("is the statement, where picking its row put the cursor, on a name it binds, or anywhere else in it", () => {
    const text = sample("guarded-api");
    const graph = holonGraph(text);
    const ok = graph.bands[0].rows.find((r) => r.name === "ok")!;
    // Its own name: one the text binds.
    const named = textCase(text, ok.at.to, graph, inspectAt(text, ok.at.to));
    expect(named?.case === "statement" && named.row.name).toBe("ok");
    // Picked, where the cursor is put (the end of its mark), though a name the language knows is
    // there: `shop.exec`.
    const og = holonGraph(orders);
    const b = og.bands.findIndex((band) => band.rows.some((r) => r.name === "shop.exec"));
    const i = og.bands[b].rows.findIndex((r) => r.name === "shop.exec");
    const exec = og.bands[b].rows[i];
    const picked = textCase(orders, exec.at.to, og, inspectAt(orders, exec.at.to), { band: b, index: i, to: exec.at.to });
    expect(picked?.case === "statement" && picked.row.name).toBe("shop.exec");
    expect(textCase(orders, exec.at.to, og, inspectAt(orders, exec.at.to))?.case, "not picked: the name").toBe("word");
    // A name the text binds, used further on: the statement it is used in.
    const used = text.indexOf("ok ?", ok.at.to) + 1;
    const later = textCase(text, used, graph, inspectAt(text, used));
    expect(later?.case).toBe("statement");
    expect(later?.case === "statement" && later.row.name).toBe("return");
    // In a string: the statement.
    expect(at(orders, '"insert into', 3)?.case).toBe("statement");
  });

  it("is nothing outside every statement and import", () => {
    const text = sample("hello-api");
    expect(textCase(text, 2, holonGraph(text), inspectAt(text, 2))).toBeNull();
  });
});
