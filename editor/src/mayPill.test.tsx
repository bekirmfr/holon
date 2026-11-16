// The It may strip's pills: every kind of thing a flow may do has an icon of its own and a short
// name, and its whole sentence is one hover away.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { describeItem } from "./holon";
import { MayStrip, stripPills } from "./MayStrip";
import { DRAWN_KINDS, pillOf, WRITES } from "./mayPill";

// Core's words, through the wasm package as the editor has them.
initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

/** Every kind core says in words (holon-author's describe.rs, read as it is written): those that
 *  take an argument, from its table (`("HttpRequest", |a| …)`), and those that take none, from its
 *  match (`"Clock" => "read the clock"`). */
function coreKinds(): { named: string[]; bare: string[] } {
  const rust = readFileSync(join(process.cwd(), "..", "crates", "holon-author", "src", "describe.rs"), "utf8");
  const body = rust.slice(rust.indexOf("pub fn describe_item("), rust.indexOf("/// Where a secret is sent"));
  const named = [...body.matchAll(/^\s*\("([A-Z][A-Za-z]+)", \|a\|/gm)].map((m) => m[1]);
  const bare = [...body.matchAll(/^\s*"([A-Z][A-Za-z]+)" => "/gm)].map((m) => m[1]);
  return { named, bare };
}

/** Guards: every kind of thing a flow may do has an icon of its own in the It may strip, and a
 *  short name; none falls to the dot of a kind the editor doesn't know.
 *  Why: the strip said each item as a whole sentence in a chip, one a line long for a secret; as
 *  pills, a kind with no icon would show as a dot and its raw item, and nothing would say so.
 *  If it fails: core says a kind (describe.rs) that mayPill.tsx doesn't draw, or the other way
 *  round. Give it an icon there (KINDS for one with an argument, BARE for one without). */
describe("every kind of thing a flow may do", () => {
  const { named, bare } = coreKinds();

  it("is read from core's own table", () => {
    // The sweep found the table and the match: both are there, and core says each in words.
    expect(named.length).toBeGreaterThanOrEqual(30);
    expect(bare.length).toBeGreaterThanOrEqual(8);
    for (const kind of named) expect(describeItem(`${kind}("x")`), kind).not.toBe(`${kind}("x")`);
    for (const kind of bare) expect(describeItem(kind), kind).not.toBe(kind);
  });

  it("has an icon and a short name", () => {
    const undrawn = [...named.map((kind) => `${kind}("x")`), ...bare].filter((item) => pillOf(item).thing === "other");
    expect(undrawn).toEqual([]);
    // And nothing is drawn that core doesn't say.
    expect(DRAWN_KINDS.filter((kind) => !named.includes(kind) && !bare.includes(kind))).toEqual([]);
    // Short: a name, never the sentence.
    for (const item of ['HttpRequest("api.example.com")', 'SecretRead("stripe_key")', 'DbWrite("shop/orders")', 'MailSend("ops/*")', 'Approval("payout/person,person")', 'RunsAction("billing/charge")', "NetworkIngress", "Limit(module_fuel=50000000)", "Limit(keep_runs=1000)", "Limit(max_requests=80)"]) {
      const { label } = pillOf(item);
      expect(label.length, `${item}: ${label}`).toBeLessThanOrEqual(32);
      expect(label, item).not.toBe(describeItem(item));
    }
    expect(pillOf('DbWrite("shop/orders")').label).toBe("shop.orders");
    expect(pillOf('MailSend("ops/*")').label).toBe("ops → any address");
    expect(pillOf('Approval("payout/person,person")').label).toBe("payout");
    expect(pillOf("Limit(keep_runs=1000)")).toEqual({ thing: "history", label: "keeps 1000 runs" });
    expect(pillOf("Limit(max_requests=80)")).toEqual({ thing: "gauge", label: "max requests 80" });
  });

  it("is told from every other by its icons", () => {
    const drawn = new Map<string, string>();
    for (const item of [...named.map((kind) => `${kind}("x")`), ...bare]) {
      const { thing, does } = pillOf(item);
      const icons = `${thing}${does ? `+${does}` : ""}`;
      expect(drawn.get(icons), `${item} is drawn as ${drawn.get(icons)} is: ${icons}`).toBeUndefined();
      drawn.set(icons, item);
    }
  });

  it("falls to the dot and the item as written when the editor doesn't know it", () => {
    expect(pillOf('Teleport("mars")')).toEqual({ thing: "other", label: 'Teleport("mars")' });
    expect(pillOf("Teleport")).toEqual({ thing: "other", label: "Teleport" });
  });
});

describe("the It may strip", () => {
  const sent = { k: ["made:@callers"], t: ["auth.example.com", "answer:@callers"] };

  it("shows each item as a pill, its sentence in its title and for a screen reader", () => {
    const strip = renderToStaticMarkup(
      <MayStrip
        deployed
        sent={sent}
        items={[
          { item: 'SecretRead("t")', change: "new" },
          { item: 'HttpRequest("auth.example.com")', change: "same" },
          { item: 'SecretRead("old")', change: "gone" },
        ]}
      />,
    );
    const sentence = "read the secret &quot;t&quot;, and send it to https://auth.example.com; what is answered to it goes to its callers";
    expect(strip).toContain(`title="${sentence} (new)"`);
    expect(strip).toContain(`<span class="hl-may__said">${sentence}</span>`);
    expect(strip).toContain('<span class="hl-may__label" aria-hidden="true">t</span>');
    expect(strip).toContain('data-thing="key"');
    expect(strip).toContain('data-does="send"');
    expect(strip).toContain('title="call https://auth.example.com"');
    expect(strip).toContain('<span class="hl-may__label" aria-hidden="true">auth.example.com</span>');
    // A gone secret: its words alone, and no arrow (what it was sent to isn't the draft's to say).
    expect(strip).toContain('title="read the secret &quot;old&quot; (no longer)"');
    expect(strip.match(/data-does="send"/g)?.length).toBe(1);
  });

  it("joins a thing's read and its write into one pill, when both changed alike", () => {
    const items = [
      { item: 'StateRead("count")', change: "same" as const },
      { item: "Clock", change: "same" as const },
      { item: 'StateWrite("count")', change: "same" as const },
      { item: 'CollectionRead("orders")', change: "same" as const },
      { item: 'CollectionWrite("orders")', change: "new" as const },
      { item: 'FileWrite("out")', change: "same" as const },
    ];
    const pills = stripPills(items);
    expect(pills.map((p) => [p.items.join(" + "), p.pill.does ?? "", p.pill.label])).toEqual([
      ['StateRead("count") + StateWrite("count")', "both", "count"],
      ["Clock", "", "clock"],
      // Read as deployed, written only by the draft: two pills, so the new one is seen.
      ['CollectionRead("orders")', "", "orders"],
      ['CollectionWrite("orders")', "write", "orders"],
      // Written and never read.
      ['FileWrite("out")', "write", "out"],
    ]);
    expect(pills[0].said).toBe('read its state "count"; write its state "count"');
    // Every item stands in exactly one pill.
    expect(pills.flatMap((p) => p.items).sort()).toEqual(items.map((m) => m.item).sort());
    for (const [write, read] of Object.entries(WRITES)) expect(pillOf(`${write}("x")`).thing, write).toBe(pillOf(`${read}("x")`).thing);
  });
});
