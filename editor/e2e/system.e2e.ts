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
import { ADMIN, expect, PORTS, RUN_DIR, SERVER, test } from "./fixtures";

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

test("approved-payout waits for its steps holding nothing, an agent's token classifying, a person reviewing and the admin approving through the run file, and pays from the balance as it is then: a deposit made meanwhile is kept", async ({ adminToken }) => {
  const base = `http://127.0.0.1:${await cliDeploy(adminToken, "approved-payout.holon.ts", "approved-payout")}`;
  const post = (path: string, data: unknown) =>
    fetch(`${base}${path}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(data) });
  const control = (token: string, path: string, data?: unknown, method?: string) =>
    fetch(`${SERVER}${path}`, {
      method: method ?? (data === undefined ? "GET" : "POST"),
      headers: { Authorization: `Bearer ${token}`, "content-type": "application/json" },
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
  type Decided = { outcome: string; slot: number; slots: { by: string[] }[] };
  const decide = (token: string, run: string, choice: { approve: boolean; reason?: string; note?: unknown }) =>
    control(token, `/deployments/approved-payout/runs/${run}/decide`, { name: "payout", ...choice });
  /** A logged value as plain JSON (wire.rs `SValue`: `{type, value}`, a record's fields as pairs). */
  type Logged = { type: string; value: unknown };
  const plain = (v: Logged): unknown =>
    v.type === "Record"
      ? Object.fromEntries((v.value as [string, Logged][]).map(([k, x]) => [k, plain(x)]))
      : v.type === "List"
        ? (v.value as Logged[]).map(plain)
        : v.value;
  /** The decision each resumed part of `run` was given, as its log entry holds it. */
  const given = async (run: string) =>
    ((await (await control(adminToken, "/deployments/approved-payout/log")).json()) as { entries: { kind: string; run_id: string; events?: Record<string, Logged> }[] }).entries
      .filter((e) => e.kind === "resume" && e.run_id === run && e.events?.payout !== undefined)
      .map((e) => plain(e.events!.payout));

  // Who takes each step, as the sample's comment sets it up: its three roles, an agent of this
  // run's own with a token, a person the admin invites, who joins by the link's token, and the
  // admin, who set the server up; each bound to one role on the tag payments, which the sample is
  // deployed with (the admin once: a retry finds it bound).
  for (const [role, kind] of [["classifier", "agent"], ["reviewer", "person"], ["approver", "person"]]) {
    const defined = await control(adminToken, `/roles/${role}`, { allows: ["run.decide"], kinds: [kind], why: "e2e: approved-payout" }, "PUT");
    expect(defined.ok, await defined.clone().text()).toBe(true);
  }
  const n = Date.now();
  const bind = async (who: string, role: string) => {
    const bound = await control(adminToken, "/bindings", { who: { principal: who }, role, scope: { flows: { tag: "payments" } } });
    expect(bound.ok, await bound.clone().text()).toBe(true);
    return ((await bound.json()) as { binding: { id: number } }).binding.id;
  };
  const scout = `e2e-scout-${n}`;
  expect((await control(adminToken, "/people", { name: scout, kind: "agent", role: "agent" })).ok).toBe(true);
  const made = await control(adminToken, `/people/${scout}/tokens`, { label: "e2e" });
  expect(made.ok, await made.clone().text()).toBe(true);
  const scoutToken = ((await made.json()) as { token: string }).token;
  const person = async (name: string) => {
    const invited = await control(adminToken, "/people", { name, kind: "person", role: "viewer" });
    expect(invited.ok, await invited.clone().text()).toBe(true);
    const { invite } = (await invited.json()) as { invite: string };
    const joined = await fetch(`${SERVER}/auth/invite`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ token: invite, password: "e2e-second-password-0123" }),
    });
    expect(joined.ok).toBe(true);
    return ((await joined.json()) as { token: string }).token;
  };
  const [reviewer, approver] = [`e2e-reviewer-${n}`, ADMIN.name];
  const reviewerToken = await person(reviewer);
  const bound = ((await (await control(adminToken, `/bindings?who=${approver}&role=approver`)).json()) as { bindings: { id: number }[] }).bindings;
  const through = { [scout]: await bind(scout, "classifier"), [reviewer]: await bind(reviewer, "reviewer"), [approver]: bound[0]?.id ?? (await bind(approver, "approver")) };

  const before = await balance();
  const earlier = await waiting();
  const asked = await post("/payout/1", { amount: 30 });
  expect(asked.status).toBe(202);
  expect(((await asked.json()) as { waiting: string }).waiting).toBe("classifier, then reviewer, then approver");
  const run = await parked(earlier);
  // Only whoever is bound to the open step's role takes it: not the admin, the approver, before
  // the review, and not the reviewer before the classifier.
  expect((await decide(adminToken, run, { approve: true })).status).toBe(403);
  expect((await decide(reviewerToken, run, { approve: true })).ok).toBe(false);
  // The agent classifies it with its token, and a note the run is given.
  const note = { label: "invoice", confidence: 0.93 };
  const classified = (await (await decide(scoutToken, run, { approve: true, note })).json()) as Decided;
  expect([classified.outcome, classified.slot]).toEqual(["waiting", 0]);
  // While it waits, a deposit goes ahead: the waiting run holds nothing on the account.
  const deposited = await post("/deposit/1", { amount: 50 });
  expect(((await deposited.json()) as { balance: number }).balance).toBe(before + 50);
  expect(((await (await decide(reviewerToken, run, { approve: true })).json()) as Decided).outcome).toBe("waiting");
  // The admin approves it on the server's machine: holon decide with no token signs in through
  // the run file (local.rs), as the account that set the server up.
  const local = { ...process.env, HOLON_RUN_DIR: RUN_DIR, HOLON_TOKEN: "", HOLON_SERVER: "" };
  const decided = execFileSync(HOLON, ["decide", "approved-payout", run, "payout", "--approve", "--json", "--server", String(PORTS.server)], { encoding: "utf8", env: local });
  const all = JSON.parse(decided) as Decided;
  expect(all.outcome).toBe("approved");
  expect(all.slots.map((s) => s.by)).toEqual([[scout], [reviewer], [approver]]);
  await expect.poll(balance).toBe(before + 50 - 30);
  // The run was given every decision: its step, who took it, of which kind and as what, the
  // classifier's note, and what let each, the binding and the access version it was read in.
  const took = (slot: number, who: string, kind: string, role: string, more: object = {}) =>
    expect.objectContaining({ slot, who, kind, role, approved: true, binding: through[who], access: expect.any(Number), ...more });
  await expect.poll(() => given(run)).toEqual([
    expect.objectContaining({
      approved: true,
      by: [scout, reviewer, approver],
      decisions: [took(0, scout, "agent", "classifier", { note }), took(1, reviewer, "person", "reviewer"), took(2, approver, "person", "approver")],
    }),
  ]);

  // One decline, with a reason, ends a payout: nothing is paid.
  const meanwhile = await waiting();
  await post("/payout/1", { amount: 30 });
  const declined = await parked(meanwhile);
  expect((await decide(scoutToken, declined, { approve: true })).ok).toBe(true);
  const no = (await (await decide(reviewerToken, declined, { approve: false, reason: "not this week" })).json()) as Decided;
  expect(no.outcome).toBe("declined");
  await expect.poll(async () => (await waiting()).includes(declined)).toBe(false);
  expect(await balance()).toBe(before + 50 - 30);
  const declining = expect.objectContaining({ slot: 1, who: reviewer, kind: "person", role: "reviewer", approved: false, reason: "not this week", binding: through[reviewer] });
  await expect.poll(() => given(declined)).toEqual([
    expect.objectContaining({ approved: false, by: [scout], declined_by: reviewer, reason: "not this week", decisions: [took(0, scout, "agent", "classifier"), declining] }),
  ]);
});
