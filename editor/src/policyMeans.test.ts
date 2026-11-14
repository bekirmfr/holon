// What the policy means (policyMeans.ts, web-console-L4.md §9.3): a sentence for every limit the
// host knows, no more, read against holon-author's LIMIT_NAMES so a new limit can't go unsaid; and
// a waiting version's items checked against the policy as the server takes them.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { checkItem, checkWaiting, LIMIT_MEANINGS, versionName } from "./policyMeans";
import type { HostPolicy, Proposal } from "./server";

/** holon-author's LIMIT_NAMES, as its source lists them (the two module limits by their names). */
function limitNames(): string[] {
  const lib = readFileSync(join(process.cwd(), "..", "crates", "holon-author", "src", "lib.rs"), "utf8");
  const list = /pub const LIMIT_NAMES: \[&str; \d+\] = \[([\s\S]*?)\];/.exec(lib)![1];
  const named = [...list.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
  const module = readFileSync(join(process.cwd(), "..", "crates", "holon-core", "src", "module.rs"), "utf8");
  const of = (c: string) => new RegExp(`pub const ${c}: &str = "([a-z_]+)"`).exec(module)![1];
  return [...named, of("FUEL_LIMIT"), of("MEMORY_LIMIT")];
}

const policy: HostPolicy = {
  defaults: { max_requests: 50, run_budget_ms: 25000 },
  ceilings: { max_requests: 500, run_budget_ms: 300000 },
  auto: ["HttpRequest(*)", 'StateRead("count")'],
  allow_plain_http: false,
  allow_private_addresses: false,
  allow_unrecorded: true,
};

const proposal = (over: Partial<Proposal>): Proposal => ({ id: 1, slug: "refunds", unit: "u", why: "", agent: true, at_ms: 0, needs: [], state: "waiting", ...over });

describe("what the policy means", () => {
  it("says what every limit the host knows means, and none it doesn't", () => {
    const names = limitNames();
    expect(names.length).toBeGreaterThan(30);
    expect(Object.keys(LIMIT_MEANINGS).sort()).toEqual([...names].sort());
    for (const n of names) expect(LIMIT_MEANINGS[n]).toMatch(/^[A-Z].*\.$/);
  });

  it("checks an item as the server would at its deploy", () => {
    expect(checkItem("Limit(max_requests=800)", policy)).toMatchObject({ says: "refused", why: expect.stringContaining("ceiling for max_requests is 500") });
    expect(checkItem("Limit(max_requests=80)", policy)).toMatchObject({ says: "person", why: expect.stringContaining("up to its ceiling of 500") });
    expect(checkItem("Limit(max_requests=20)", policy).says).toBe("auto");
    expect(checkItem("Limit(no_such=1)", policy).says).toBe("refused");
    expect(checkItem('PlainHttp("legacy.local")', policy).says).toBe("refused");
    expect(checkItem('PlainHttp("legacy.local")', { ...policy, allow_plain_http: true }).says).toBe("person");
    expect(checkItem("PrivateAddresses", policy).says).toBe("refused");
    expect(checkItem('Unrecorded("GET /health")', policy).says).toBe("person");
    expect(checkItem('Unrecorded("GET /health")', { ...policy, allow_unrecorded: false }).says).toBe("refused");
    expect(checkItem('HttpRequest("api.stripe.com")', policy)).toMatchObject({ says: "auto", why: expect.stringContaining("HttpRequest(*)") });
    expect(checkItem('StateRead("count")', policy).says).toBe("auto");
    expect(checkItem('StateRead("other")', policy).says).toBe("person");
    // A tag is never the policy's to grant, whatever its auto says.
    expect(checkItem('Tag("payments")', { ...policy, auto: ["Tag(*)"] }).says).toBe("person");
  });

  it("checks each waiting version's asks, and only the waiting ones", () => {
    const checked = checkWaiting(
      [
        proposal({ id: 1, version: 4, needs: ['HttpRequest("api.stripe.com")', "Limit(max_requests=800)"] }),
        proposal({ id: 2, slug: "old", state: "declined", needs: ["PrivateAddresses"] }),
        proposal({ id: 3, slug: "first", needs: [] }),
      ],
      policy,
    );
    expect(checked.map((c) => [versionName(c), c.verdicts.map((v) => v.says)])).toEqual([
      ["refunds v4", ["auto", "refused"]],
      ["first's waiting version", []],
    ]);
  });
});
