// What waits for a person (ApprovalsPage.tsx), and a version reviewed before it is deployed
// (ReviewPage.tsx): a flow saved and never deployed waits under Deploys; its review says what it
// may do and shows its text; Approve deploys it once the person says they read it.

import { expect, test } from "./fixtures";
import { gotoPage } from "./helpers";

test("a flow saved and never deployed waits under Deploys; reviewed, ticked and approved, it is deployed", async ({ page }) => {
  const name = `review-${Date.now().toString(36)}`;
  await page.goto("/");
  await gotoPage(page, "new");
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
