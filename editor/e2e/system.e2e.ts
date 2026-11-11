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
import { ADMIN, expect, SERVER, test } from "./fixtures";

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

test("refund-once, beside orders-api on one database, calls the payment API once for two refunds of one order at once", async ({ adminToken }) => {
  const orders = `http://127.0.0.1:${await cliDeploy(adminToken, "orders-api.holon.ts", "orders-api")}`;
  const refunds = `http://127.0.0.1:${await cliDeploy(adminToken, "refund-once.holon.ts", "refund-once")}`;
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

test("approved-payout waits for two people's approval holding nothing, and pays from the balance as it is then: a deposit made meanwhile is kept", async ({ adminToken }) => {
  const base = `http://127.0.0.1:${await cliDeploy(adminToken, "approved-payout.holon.ts", "approved-payout")}`;
  const post = (path: string, data: unknown) =>
    fetch(`${base}${path}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(data) });
  const control = (token: string, path: string, data?: unknown, headers: Record<string, string> = {}) =>
    fetch(`${SERVER}${path}`, {
      method: data === undefined ? "GET" : "POST",
      headers: { Authorization: `Bearer ${token}`, "content-type": "application/json", ...headers },
      body: data === undefined ? undefined : JSON.stringify(data),
    });
  const balance = async () => ((await (await fetch(`${base}/accounts/1`)).json()) as { balance: number }).balance;
  type Run = { run_id: string; waiting?: Array<{ name: string }> };
  const waiting = async () =>
    ((await (await control(adminToken, "/deployments/approved-payout/runs")).json()) as { runs: Run[] }).runs
      .filter((r) => r.waiting?.some((w) => w.name === "payout"))
      .map((r) => r.run_id);
  /** The run a payout just started, once it waits at its approval. */
  const parked = async (before: string[]) => {
    let id: string | undefined;
    await expect.poll(async () => (id = (await waiting()).find((r) => !before.includes(r)))).toBeTruthy();
    return id as string;
  };
  const decide = (token: string, run: string, choice: { approve: boolean; reason?: string }, headers?: Record<string, string>) =>
    control(token, `/deployments/approved-payout/runs/${run}/decide`, { name: "payout", ...choice }, headers);

  // A second person: an editor the admin invites, who joins by the link's token.
  const name = `e2e-second-${Date.now()}`;
  const invited = await control(adminToken, "/people", { name, kind: "person", role: "editor" });
  expect(invited.ok, await invited.clone().text()).toBe(true);
  const { invite } = (await invited.json()) as { invite: string };
  const joined = await fetch(`${SERVER}/auth/invite`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token: invite, password: "e2e-second-password-0123" }),
  });
  expect(joined.ok).toBe(true);
  const second = ((await joined.json()) as { token: string }).token;

  const before = await balance();
  const earlier = await waiting();
  const asked = await post("/payout/1", { amount: 30 });
  expect(asked.status).toBe(202);
  const run = await parked(earlier);
  // An agent never decides an approval, even under an admin's name.
  expect((await decide(adminToken, run, { approve: true }, { "X-Holon-Role": "agent" })).status).toBe(403);
  // One person's approval isn't enough, and the same person twice counts once.
  const first = await decide(adminToken, run, { approve: true });
  expect(((await first.json()) as { outcome: string }).outcome).toBe("waiting");
  expect((await decide(adminToken, run, { approve: true })).status).toBe(409);
  // While it waits, a deposit goes ahead: the waiting run holds nothing on the account.
  const deposited = await post("/deposit/1", { amount: 50 });
  expect(((await deposited.json()) as { balance: number }).balance).toBe(before + 50);
  const both = (await (await decide(second, run, { approve: true })).json()) as { outcome: string; approved_by: string[] };
  expect(both.outcome).toBe("approved");
  expect(both.approved_by).toEqual([ADMIN.name, name]);
  await expect.poll(balance).toBe(before + 50 - 30);

  // One decline, with a reason, ends a payout: nothing is paid.
  const meanwhile = await waiting();
  await post("/payout/1", { amount: 30 });
  const declined = await parked(meanwhile);
  const no = (await (await decide(second, declined, { approve: false, reason: "not this week" })).json()) as { outcome: string };
  expect(no.outcome).toBe("declined");
  await expect.poll(async () => (await waiting()).includes(declined)).toBe(false);
  expect(await balance()).toBe(before + 50 - 30);
});
