// The docs tab (ManualPage.tsx): the manual, built into the editor from docs/manual. Its contents
// move between pages, a link to a heading on another page lands on that heading, and a flow in it
// opens in a tab of its own.

import { expect, test } from "./fixtures";

test("the docs tab shows the manual, follows its links and opens its flows", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "docs", exact: true }).click();
  await expect(page.getByRole("tablist", { name: "open tabs" }).getByRole("tab", { name: "docs" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { level: 1, name: "The Holon manual" })).toBeVisible();

  // The contents, as the manual's index lists them.
  const contents = page.getByRole("navigation", { name: "manual contents" });
  await contents.getByRole("button", { name: "Deploying and permissions" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Deploying and permissions" })).toBeVisible();

  // A link to a heading on another page: that page, scrolled to it.
  await page.locator('a[href="#manual-limits"]').first().click();
  await expect(page.getByRole("heading", { level: 1, name: "Permissions" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Limits", exact: true })).toBeInViewport();

  // A flow in the tour opens as an unsaved copy, in a tab of its own.
  await contents.getByRole("button", { name: "A tour" }).click();
  await page.getByRole("button", { name: "open in a tab" }).first().click();
  await expect(page.getByRole("tablist", { name: "open tabs" }).getByRole("tab", { name: /^hello/ })).toHaveAttribute("aria-selected", "true");
});
