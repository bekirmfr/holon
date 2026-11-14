// What waits for a person (ApprovalsPage.tsx), and a version reviewed before it is deployed
// (ReviewPage.tsx): a flow saved and never deployed waits under Deploys; its review says what it
// may do and shows its text; Approve deploys it once the person says they read it. Runs waiting
// lists, across flows, the runs whoever is signed in may see or decide, an approval in steps as
// its chain, and the run's page what each step's decision was.

import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Browser } from "@playwright/test";
import { expect, HOLON, PORTS, SERVER, test } from "./fixtures";
import { newFlow } from "./helpers";

test("a flow saved and never deployed waits under Deploys; reviewed, ticked and approved, it is deployed", async ({ page }) => {
  const name = `review-${Date.now().toString(36)}`;
  await page.goto("/");
  await newFlow(page);
  const canvas = page.locator(".hl-canvas");
  await canvas.getByRole("button", { name: "text off" }).click();
  await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(`import { action, flow } from "holon"\n\nflow("${name}")\n\nexport const shout = action((input) => input.toUpperCase())\n`);
  await expect(page.locator(".hl-canvas__flow > .react-flow .react-flow__node", { hasText: "uppercase" })).toHaveCount(1, { timeout: 15_000 });
  await canvas.getByRole("button", { name: "save", exact: true }).dispatchEvent("click");
  await page.locator('[aria-modal="true"] input').fill(name);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("region", { name: "notices" })).toContainText("saved");

  // Approvals lists it, and the nav counts what waits.
  await page.getByRole("navigation", { name: "main" }).getByRole("link", { name: /^Approvals/ }).click();
  await expect(page).toHaveURL(/#approvals$/);
  await expect(page.locator(".hl-nav__badge")).toHaveText(/^\d+$/);
  const row = page.getByRole("region", { name: "deploys" }).getByRole("row").filter({ hasText: name });
  await expect(row).toContainText("first deploy");
  await row.getByRole("link", { name: `review ${name}` }).click();

  // Its review: what it may do, its text, and Approve held until the person says they read it.
  await expect(page).toHaveURL(new RegExp(`#approvals\\.${name}\\.v1$`));
  await expect(page.getByRole("region", { name: "what it may do" })).toBeVisible();
  const review = page.getByRole("region", { name: "review" });
  await review.getByRole("button", { name: "Whole flow" }).click();
  await expect(review.getByRole("region", { name: "whole flow" })).toContainText("input.toUpperCase()");
  const decision = page.getByRole("complementary", { name: "decision" });
  await expect(decision).toContainText("nothing: its first deploy");
  const approve = decision.getByRole("button", { name: /^(Approve and deploy|Deploy)$/ });
  await expect(approve).toBeDisabled();
  await decision.getByRole("checkbox").check();
  await approve.click();

  // Deployed: its page says so, and nothing waits for it any more.
  await expect(page).toHaveURL(new RegExp(`#${name}$`));
  await expect(page.getByLabel(`flow ${name}`).locator(".hl-flow__head .hl-chip").first()).toHaveText("live");
  await page.goto("/#approvals");
  await expect(page.getByRole("region", { name: "deploys" }).getByRole("row").filter({ hasText: name })).toHaveCount(0);
});

/** The editor, in a browser of its own, signed in with `token` (as signing in leaves it). */
async function editorAs(browser: Browser, token: string, name: string) {
  const context = await browser.newContext({ baseURL: `http://localhost:${PORTS.editor}` });
  await context.addInitScript((session) => localStorage.setItem("holon.session", session), JSON.stringify({ token, name, kind: "person", role: "viewer" }));
  return context.newPage();
}

test("Runs waiting lists across flows an approval in steps as its chain: an agent takes its step through the API, a viewer sees it with no buttons, a person bound only at the approval decides it here, a reason required to decline, and the run's page says each decision", async ({
  page,
  browser,
  adminToken,
}) => {
  const n = Date.now().toString(36);
  const slug = `chain-${n}`;
  const control = (token: string, path: string, data?: unknown, method?: string) =>
    fetch(`${SERVER}${path}`, {
      method: method ?? (data === undefined ? "GET" : "POST"),
      headers: { Authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: data === undefined ? undefined : JSON.stringify(data),
    });
  const sure = async (res: Promise<Response>) => {
    const r = await res;
    expect(r.ok, await r.clone().text()).toBe(true);
    return r;
  };
  // Its two steps' roles: an agent classifies, a person approves.
  for (const [role, kind] of [["e2e-classifier", "agent"], ["e2e-approver", "person"]]) {
    await sure(control(adminToken, `/roles/${role}`, { allows: ["run.decide"], kinds: [kind], why: "e2e: Runs waiting" }, "PUT"));
  }
  const file = join(tmpdir(), `${slug}.holon.ts`);
  writeFileSync(
    file,
    `import { action, approval, flow } from "holon"

flow("${slug}")

// Waits for an agent to classify it, then a person to approve it.
export const pay = action(async (order) => {
  const decision = await approval("payout", {
    steps: [{ role: "e2e-classifier", kind: "agent" }, { role: "e2e-approver" }],
    expires: "1h",
    shows: { amount: order.amount },
  })
  return decision.approved
})
`,
  );
  execFileSync(HOLON, ["deploy", file, "--server", SERVER, "--slug", slug, "--approve"], { env: { ...process.env, HOLON_TOKEN: adminToken }, stdio: "pipe" });

  // scout, an agent with a token, classifies on the flow; dee, joined by an invite, holds nothing
  // on the host and approves at the approval point alone; vic is a viewer on the host.
  const scout = `e2e-scout-${n}`;
  await sure(control(adminToken, "/people", { name: scout, kind: "agent", role: "agent" }));
  const scoutToken = ((await (await sure(control(adminToken, `/people/${scout}/tokens`, { label: "e2e" }))).json()) as { token: string }).token;
  await sure(control(adminToken, "/bindings", { who: { principal: scout }, role: "e2e-classifier", scope: { flows: { slug } } }));
  const joined = async (name: string) => {
    const { invite } = (await (await sure(control(adminToken, "/people", { name, kind: "person", role: "viewer" }))).json()) as { invite: string };
    const accepted = await sure(fetch(`${SERVER}/auth/invite`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token: invite, password: "e2e-second-password-0123" }) }));
    return ((await accepted.json()) as { token: string }).token;
  };
  const [dee, vic] = [`e2e-dee-${n}`, `e2e-vic-${n}`];
  const [deeToken, vicToken] = [await joined(dee), await joined(vic)];
  await sure(control(adminToken, "/bindings", { who: { principal: dee }, role: "e2e-approver", scope: { approval: { flow: { slug }, name: "payout" } } }));
  const held = ((await (await sure(control(adminToken, `/bindings?who=${dee}`))).json()) as { bindings: { id: number; scope: unknown }[] }).bindings;
  for (const b of held.filter((b) => b.scope === "host")) await sure(control(adminToken, `/bindings/${b.id}?why=e2e`, undefined, "DELETE"));
  // dee lists no flow, and still finds the runs waiting for them.
  expect((await control(deeToken, "/deployments")).status).toBe(403);

  // A run waits at it; scout classifies it through the API, with a note.
  expect((await control(adminToken, `/trigger/${slug}/pay`, { amount: 30 })).status).toBe(202);
  type Listed = { slug: string; run_id: string };
  let run = "";
  await expect
    .poll(async () => (run = ((await (await control(deeToken, "/runs/waiting")).json()) as { runs: Listed[] }).runs.find((r) => r.slug === slug)?.run_id ?? ""))
    .not.toBe("");
  await sure(control(scoutToken, `/deployments/${slug}/runs/${run}/decide`, { name: "payout", approve: true, note: { label: "invoice", confidence: 0.93 } }));

  // vic, a viewer, sees the run and its chain, and has nothing to decide.
  const chain = `✓ e2e-classifier (${scout}, invoice, 0.93) → ◻ e2e-approver`;
  const asVic = await editorAs(browser, vicToken, vic);
  await asVic.goto("/#approvals.runs");
  const vicRow = asVic.getByRole("region", { name: "runs waiting" }).getByRole("row").filter({ hasText: slug });
  await expect(vicRow).toContainText(chain);
  await expect(vicRow.getByRole("button")).toHaveCount(0);
  await expect(vicRow.getByRole("link", { name: slug, exact: true })).toHaveCount(1);
  await asVic.context().close();

  // dee sees it counted in the nav, the step scout took with its decision on hover, and theirs.
  const asDee = await editorAs(browser, deeToken, dee);
  await asDee.goto("/#approvals.runs");
  await expect(asDee.locator(".hl-nav__badge")).toHaveText("1");
  const row = asDee.getByRole("region", { name: "runs waiting" }).getByRole("row").filter({ hasText: slug });
  await expect(row).toContainText(chain);
  await expect(row).toContainText("shows: amount 30");
  // A flow they may not open is named, not linked.
  await expect(row.getByRole("link", { name: slug, exact: true })).toHaveCount(0);
  await expect(row.locator(".hl-chain__step--done")).toHaveAttribute("title", new RegExp(`: ${scout} \\(an agent\\) approved as e2e-classifier · noted \\{"confidence":0\\.93,"label":"invoice"\\} · through binding \\d+ \\(holon access show --version \\d+\\)$`));
  // Declining needs a reason: the button waits for one.
  const dialog = asDee.locator('[aria-modal="true"]');
  await row.getByRole("button", { name: new RegExp(`^decline "payout" of ${slug}`) }).click();
  await expect(dialog.getByRole("button", { name: "Decline" })).toBeDisabled();
  await expect(dialog).toContainText("A decline says why: type a reason.");
  await asDee.keyboard.press("Escape");
  await row.getByRole("button", { name: new RegExp(`^approve "payout" of ${slug}`) }).click();
  await dialog.locator("input").fill("checked the amount");
  await dialog.getByRole("button", { name: "Approve" }).click();
  await expect(asDee.getByRole("region", { name: "notices" })).toContainText(`Approved by ${scout} and ${dee}: the run goes on.`);
  await expect(row).toHaveCount(0);
  await asDee.context().close();

  // The run's page, for the admin: what it was given, and each step's decision a line.
  type Entry = { seq: number; kind: string; run_id: string };
  let seq = 0;
  await expect
    .poll(async () => (seq = ((await (await control(adminToken, `/deployments/${slug}/log`)).json()) as { entries: Entry[] }).entries.find((e) => e.kind === "resume" && e.run_id === run)?.seq ?? 0))
    .toBeGreaterThan(0);
  await page.goto(`/#${slug}.run-${seq}`);
  const decided = page.getByRole("region", { name: "decided" });
  await expect(decided).toContainText(`approved by ${scout} and ${dee}`);
  await expect(decided).toContainText(new RegExp(`${scout} \\(an agent\\) approved as e2e-classifier · noted .* · through binding \\d+`));
  await expect(decided).toContainText(new RegExp(`${dee} approved as e2e-approver: checked the amount · through binding \\d+ \\(holon access show --version \\d+\\)`));
});
