// The address says where the editor is (routes.ts): a reload keeps the place, Back goes back, a
// flow's address opens it, and an address that names nothing says so.

import { expect, test } from "./fixtures";
import { deploy, gotoPage, notices, openSample, viewNewestRun } from "./helpers";

test("a reload keeps the page, the flow and its mode; Back goes back", async ({ page }) => {
  await page.goto("/#flows");
  const tabs = page.getByRole("tablist", { name: "open tabs" });
  await expect(tabs.getByRole("tab", { name: "flows" })).toHaveAttribute("aria-selected", "true");
  await gotoPage(page, "deployed");
  await expect(page).toHaveURL(/#deployed$/);
  await page.goBack();
  await expect(page).toHaveURL(/#flows$/);
  await expect(tabs.getByRole("tab", { name: "flows" })).toHaveAttribute("aria-selected", "true");

  // A flow open in the editor: its address, kept over a reload once it is saved somewhere the
  // editor can find it (here, deployed).
  await openSample(page, "Hello API", false);
  await expect(page).toHaveURL(/#editor\.hello-api$/);
  await deploy(page);
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).dispatchEvent("click");
  await expect(page).toHaveURL(/#editor\.hello-api\.view(\.run-\d+)?$/);
  await page.reload();
  await expect(page.locator(".hl-canvas--viewing")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".hl-tab--active")).toContainText("hello-api");

  // The address of edit mode, typed: the same flow, editable.
  await page.goto("/#editor.hello-api");
  await expect(page.locator(".hl-tab--active")).toContainText("hello-api", { timeout: 30_000 });
  await expect(page.locator(".hl-canvas--viewing")).toHaveCount(0);
  await expect(page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true })).toBeVisible();
});

test("the manual's page is in the address", async ({ page }) => {
  await page.goto("/");
  await gotoPage(page, "docs");
  await expect(page).toHaveURL(/#docs$/);
  const contents = page.getByRole("navigation", { name: "manual contents" });
  await contents.getByRole("button").nth(2).click();
  await expect(page).toHaveURL(/#docs\.[^@]+/);
  const at = page.url();
  await page.reload();
  await expect(page).toHaveURL(at);
  await page.goBack();
  await expect(page).toHaveURL(/#docs$/);
});

test("keys: a page by its number, Esc up, ? for them all, and j steps through a run", async ({ page, request }) => {
  await page.goto("/#deployed");
  // Once the editor is there to hear them.
  await expect(page.getByRole("tablist", { name: "open tabs" }).getByRole("tab", { name: "deployed" })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("3");
  await expect(page).toHaveURL(/#flows$/);
  await page.keyboard.press("Escape");
  await expect(page).toHaveURL(/#overview$/);
  await page.keyboard.press("3");
  await expect(page).toHaveURL(/#flows$/);
  await page.keyboard.press("1");
  await expect(page).toHaveURL(/#overview$/);

  await page.keyboard.press("?");
  const keys = page.getByRole("dialog", { name: "keys" });
  await expect(keys).toContainText("step forward one node");
  // While it is open, the number keys are its, not the pages'.
  await page.keyboard.press("3");
  await expect(page).toHaveURL(/#overview$/);
  await page.keyboard.press("Escape");
  await expect(keys).toHaveCount(0);

  // A run, stepped through from the keys.
  await openSample(page, "Hello API", false);
  const port = await deploy(page);
  expect((await request.get(`http://127.0.0.1:${port}/hello/Ada`)).status()).toBe(200);
  await viewNewestRun(page);
  await page.locator(".hl-view__bottom").getByRole("button", { name: "Timeline" }).click();
  await page.keyboard.press("j");
  await expect(page.locator(".hl-view__bottom")).toContainText(/node 1 of \d+/);
  await page.keyboard.press("j");
  await expect(page.locator(".hl-view__bottom")).toContainText(/node 2 of \d+/);
  await page.keyboard.press("End");
  await expect(page.locator(".hl-view__bottom")).toContainText(/all \d+ nodes/);
  await expect(page.getByRole("slider", { name: "step" })).toBeVisible();
});

test("an address that names nothing says so, and shows the overview", async ({ page }) => {
  await page.goto("/#no.such.place");
  await expect(notices(page)).toContainText("Nothing is at #no.such.place");
  await expect(page).toHaveURL(/#overview$/);
});
