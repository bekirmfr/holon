// The Overview (OverviewPage.tsx), from the host's summary (GET /summary, holon-server's
// summary.rs): the pulse sample deployed, ticked and asked for a beat not come yet (a 503: a run
// that failed), its runs hour by hour in the flows table, red where one failed, its newest runs
// among the latest across every flow, and Check now reading every log's chain again.

import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, SERVER, test } from "./fixtures";
import { navLink } from "./helpers";

const root = join(fileURLToPath(import.meta.url), "..", "..", "..");
const HOLON = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
const PULSE = join(root, "crates", "holon-author", "samples", "pulse.holon.ts");

const holon = (token: string, args: string[]) => execFileSync(HOLON, [...args, "--server", SERVER], { env: { ...process.env, HOLON_TOKEN: token }, encoding: "utf8", stdio: "pipe" });
/** `holon`, where it may fail (a request it sends answers 503): whether it did. */
const tried = (token: string, args: string[]) => {
  try {
    holon(token, args);
    return true;
  } catch {
    return false;
  }
};

test("the overview counts pulse's runs by hour, red where one failed, lists its newest, and checks the run log", async ({ page, request, adminToken }) => {
  const slug = `pulse-${Date.now().toString(36)}`;
  holon(adminToken, ["deploy", PULSE, "--slug", slug, "--approve"]);
  // Its clock, by hand, so it has beats now; the first beat answers, one not come yet is a 503.
  for (let i = 0; i < 3; i++) holon(adminToken, ["tick", slug, "every 5s"]);
  expect(tried(adminToken, ["call", slug, "GET", "/beat/1"])).toBe(true);
  expect(tried(adminToken, ["call", slug, "GET", "/beat/100000"])).toBe(false);
  try {
    await page.goto("/#overview");
    await expect(navLink(page, "Overview")).toHaveAttribute("aria-current", "page");
    await expect(page.getByRole("heading", { level: 1, name: /^Good (morning|afternoon|evening), e2e$/ })).toBeVisible();
    const glance = page.getByRole("region", { name: "at a glance" });
    await expect(glance).toContainText("runs in the last 24 h");
    await expect(glance).toContainText("of them failed");

    // Its row: live, its last 24 hours as bars (read out as words), the 503 among them failed.
    const row = page.getByRole("region", { name: "flows" }).getByRole("row").filter({ hasText: slug });
    await expect(row).toContainText("live");
    await expect(row.getByRole("img")).toHaveAccessibleName(/^\d+ runs, 1 failed in the last 24 hours$/);
    await expect(row.locator(".hl-spark__bad")).toHaveCount(1);
    await expect.poll(async () => Number((await row.locator("td").last().innerText()).replace(/,/g, ""))).toBeGreaterThanOrEqual(5);

    // Its newest runs, among the latest across every flow: one opens its record.
    const latest = page.getByRole("region", { name: "latest runs" });
    await latest.getByRole("link", { name: new RegExp(`^${slug} run \\d+$`) }).first().click();
    await expect(page).toHaveURL(new RegExp(`#${slug}\\.run-\\d+$`));

    // Check now reads every log again; the run log card says what it found.
    await page.goto("/#overview");
    await page.getByRole("button", { name: "Check now" }).click();
    await expect(page.getByText(/^Checked [\d,]+ runs: every line names the one before it\.$/)).toBeVisible();
    await expect(page.getByRole("region", { name: "run log" })).toContainText("every line names the one before");
    await expect(glance).toContainText(/run log, checked \d/);
  } finally {
    await request.delete(`${SERVER}/deployments/${slug}`, { headers: { Authorization: `Bearer ${adminToken}` } });
  }
});
