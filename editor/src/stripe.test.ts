// The stripe_verify template, run through the real core (the wasm package, loaded from disk):
// a request Stripe signed passes; a changed body, a stale timestamp, a wrong secret, or no
// header at all doesn't. The signatures are computed here with Node's own crypto, independently
// of core's hmac_sha256.

import { createHmac } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { initSync, Evaluator, primitive_catalog } from "../wasm/holon_wasm.js";
import { buildPreimage, type WUnit, type WValue } from "./holon";
import { buildTemplate, templateDefaults } from "./templates";

const SECRET = "whsec_test_0123456789";
const NOW = 1_792_000_000;

let ev: Evaluator;
let verify: Uint8Array;

beforeAll(() => {
  initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });
  ev = new Evaluator();
  const catalog = new Map<string, WUnit>();
  for (const u of primitive_catalog() as WUnit[]) {
    if (u.body.kind === "Primitive") catalog.set(u.body.name, u);
  }
  const unit = buildTemplate("stripe_verify", templateDefaults("stripe_verify"), catalog, (c) => ev.insert(buildPreimage(c)));
  if (typeof unit === "string") throw new Error(unit);
  verify = buildPreimage(unit);
});

const text = (value: string): WValue => ({ type: "Text", value });
const record = (fields: Record<string, WValue>): WValue => ({
  type: "Record",
  value: Object.entries(fields).sort(([a], [b]) => (a < b ? -1 : 1)),
});
const sign = (secret: string, t: number, body: string) =>
  createHmac("sha256", secret).update(`${t}.${body}`).digest("hex");

/** The request record the data plane builds, with `headers` as given. */
const request = (body: string, headers: Record<string, string>): WValue =>
  record({
    method: text("POST"),
    path: text("/stripe"),
    params: record({}),
    query: record({}),
    headers: record(Object.fromEntries(Object.entries(headers).map(([k, v]) => [k, text(v)]))),
    body: text(body),
  });

function run(req: WValue, secret = SECRET, now = NOW): [WValue | null, WValue | null] {
  // An absent output crosses the bridge as undefined.
  const out = ev.eval(verify, [req, text(secret), { type: "Number", value: now }]) as (WValue | undefined)[];
  return [out[0] ?? null, out[1] ?? null];
}

describe("stripe_verify", () => {
  const body = JSON.stringify({ id: "evt_1", type: "payment_intent.succeeded", data: { object: { amount: 2000 } } });
  const t = NOW - 30;
  const header = `t=${t},v1=${sign(SECRET, t, body)},v0=ignored`;

  it("passes what Stripe signed, and hands on the parsed event", () => {
    const [event, verified] = run(request(body, { "stripe-signature": header }));
    expect(verified).toEqual({ type: "Boolean", value: true });
    expect(event?.type).toBe("Record");
    const fields = Object.fromEntries((event as { value: [string, WValue][] }).value);
    expect(fields.type).toEqual(text("payment_intent.succeeded"));
  });

  it("refuses a changed body, a wrong secret, or a stale timestamp", () => {
    const tampered = run(request(body.replace("2000", "1"), { "stripe-signature": header }));
    expect(tampered).toEqual([null, { type: "Boolean", value: false }]);
    expect(run(request(body, { "stripe-signature": header }), "whsec_other_secret")[1]).toEqual({ type: "Boolean", value: false });
    const late = run(request(body, { "stripe-signature": header }), SECRET, t + 301);
    expect(late).toEqual([null, { type: "Boolean", value: false }]);
    expect(run(request(body, { "stripe-signature": header }), SECRET, t + 300)[1]).toEqual({ type: "Boolean", value: true });
  });

  it("passes when any v1 matches, wherever it is (Stripe signs with both while a secret is rolled)", () => {
    const old = sign("whsec_old_secret", t, body);
    const current = sign(SECRET, t, body);
    for (const h of [`t=${t},v1=${old},v1=${current}`, `t=${t},v1=${current},v1=${old}`, `t=${t},v0=x,v1=${current}`]) {
      expect(run(request(body, { "stripe-signature": h }))[1], h).toEqual({ type: "Boolean", value: true });
    }
    // None matching, or the right signature under another scheme's name, doesn't pass.
    for (const h of [`t=${t},v1=${old},v1=${sign("whsec_x", t, body)}`, `t=${t},v0=${current}`]) {
      expect(run(request(body, { "stripe-signature": h })), h).toEqual([null, { type: "Boolean", value: false }]);
    }
  });

  it("has nothing to say about a request without the header, or a malformed one", () => {
    expect(run(request(body, {}))).toEqual([null, null]);
    expect(run(request(body, { "stripe-signature": "garbage" }))).toEqual([null, null]);
  });
});
