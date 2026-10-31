// System-level tests for samples: compile and deploy via `holon deploy` (no browser), then assert
// over plain HTTP. These are decoupled from the editor UI so pointer-event layout changes can't
// break them, and they can run wherever the holon binary and a test server are available.
//
// Only pure-HTTP tests live here — ones that need the browser UI after deploy (view-mode replay,
// the inspector, etc.) stay in samples.e2e.ts. Each deploy approves what it asks for (`--approve`,
// signed in as the e2e admin), so these run on a server no browser test has deployed to.

import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, SERVER, test } from "./fixtures";

const root = join(fileURLToPath(import.meta.url), "..", "..", "..");
const HOLON = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
const SAMPLES = join(root, "crates", "holon-author", "samples");

/** Deploy a sample via CLI and return the port it serves on (null for clock flows). */
async function cliDeploy(token: string, file: string, slug: string): Promise<number | null> {
  execFileSync(HOLON, ["deploy", join(SAMPLES, file), "--server", SERVER, "--slug", slug, "--approve"], {
    env: { ...process.env, HOLON_TOKEN: token },
    stdio: "pipe",
  });
  const res = await fetch(`${SERVER}/deployments`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(res.ok).toBe(true);
  const { deployments } = (await res.json()) as { deployments: Array<{ slug: string; port?: number }> };
  return deployments.find((d) => d.slug === slug)?.port ?? null;
}

test("likes answers a count in a sentence: the number as text (as_text), then the words", async ({ adminToken }) => {
  const port = await cliDeploy(adminToken, "likes.holon", "likes");
  expect(port).not.toBeNull();
  const base = `http://127.0.0.1:${port}`;
  const post = `e2e-${Date.now()}`;
  expect((await fetch(`${base}/posts/${post}/like`, { method: "POST" })).status).toBe(200);
  const shown = await fetch(`${base}/posts/${post}/likes`);
  expect(shown.status).toBe(200);
  expect(await shown.text()).toBe("1 likes");
  expect(await (await fetch(`${base}/posts/${post}-never/likes`)).text()).toBe("0 likes");
});

test("counter counts its hits with no wire to the request, and records the last visit after it is put", async ({ adminToken }) => {
  const port = await cliDeploy(adminToken, "counter.holon", "counter");
  expect(port).not.toBeNull();
  const base = `http://127.0.0.1:${port}`;
  const hit = async () => Number((await (await fetch(`${base}/hit`)).text()).replace("hit ", ""));
  const first = await hit();
  expect(await hit()).toBe(first + 1);
  const who = `e2e-${Date.now()}`;
  expect(await (await fetch(`${base}/visits/${who}`, { method: "POST" })).text()).toBe(`visited ${who}`);
  expect(await (await fetch(`${base}/last`)).text()).toBe(who);
});

test("refund-once, beside orders-api on one database, calls the payment API once for two refunds of one order at once", async ({ adminToken }) => {
  const orders = `http://127.0.0.1:${await cliDeploy(adminToken, "orders-api.holon", "orders-api")}`;
  const refunds = `http://127.0.0.1:${await cliDeploy(adminToken, "refund-once.holon", "refund-once")}`;
  const customer = `e2e-refund-${Date.now()}`;
  expect((await fetch(`${orders}/orders`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ customer, total: 5 }) })).status).toBe(201);
  const [order] = (await (await fetch(`${orders}/orders/${customer}`)).json()) as { id: string }[];
  const both = await Promise.all([
    fetch(`${refunds}/refund/${order.id}`, { method: "POST" }),
    fetch(`${refunds}/refund/${order.id}`, { method: "POST" }),
  ]);
  expect(both.map((r) => r.status).sort()).toEqual([200, 409]);
  const made = (await (await fetch(`${refunds}/refunds/${order.id}`)).json()) as { amount: number }[];
  expect(made).toHaveLength(1);
  expect(made[0].amount).toBe(5);
  expect(((await (await fetch(`${orders}/order/${order.id}`)).json()) as { refunded: number }).refunded).toBe(1);
});

test("approved-payout waits for its approval holding nothing, and pays from the balance as it is then: a deposit made meanwhile is kept", async ({ adminToken }) => {
  const base = `http://127.0.0.1:${await cliDeploy(adminToken, "approved-payout.holon", "approved-payout")}`;
  const post = (path: string, data: unknown) =>
    fetch(`${base}${path}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(data) });
  const balance = async () => ((await (await fetch(`${base}/accounts/1`)).json()) as { balance: number }).balance;
  const before = await balance();
  const asked = await post("/payout/1", { amount: 30 });
  expect(asked.status).toBe(202);
  const { approve } = (await asked.json()) as { approve: string };
  expect(approve).toMatch(/^\/approve\/rt_/);
  // While it waits, a deposit goes ahead: the waiting run holds nothing on the account.
  const deposited = await post("/deposit/1", { amount: 50 });
  expect(((await deposited.json()) as { balance: number }).balance).toBe(before + 50);
  expect((await post(approve, { by: "e2e" })).status).toBe(200);
  expect(await balance()).toBe(before + 50 - 30);
});
