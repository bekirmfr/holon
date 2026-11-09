// The address says where the editor is (routes.ts): a reload keeps the place, Back goes back, a
// flow's address opens it, and an address that names nothing says so.

import { expect, test } from "./fixtures";
import { deploy, gotoPage, navLink, notices, openSample, viewNewestRun } from "./helpers";

test("a reload keeps the page, the flow and its mode; Back goes back", async ({ page }) => {
  await page.goto("/#flows");
  await expect(navLink(page, "Flows")).toHaveAttribute("aria-current", "page");
  await gotoPage(page, "library");
  await expect(page).toHaveURL(/#library$/);
  await page.goBack();
  await expect(page).toHaveURL(/#flows$/);
  await expect(navLink(page, "Flows")).toHaveAttribute("aria-current", "page");

  // A flow open in the editor: its address, kept over a reload once it is saved somewhere the
  // editor can find it (here, deployed).
  await openSample(page, "Hello API", false);
  await expect(page).toHaveURL(/#editor\.hello-api$/);
  await deploy(page);
  // e: View runs.
  await page.locator(".hl-canvas__flow").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("e");
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
  await page.goto("/#library");
  // Once the editor is there to hear them.
  await expect(navLink(page, "Library")).toHaveAttribute("aria-current", "page");
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

test("the side nav says the page, the crumbs the way up, the host card the host", async ({ page }) => {
  await page.goto("/#docs.language/tour.md");
  const nav = page.getByRole("navigation", { name: "main" });
  await expect(nav.getByRole("link", { name: "Docs", exact: true })).toHaveAttribute("aria-current", "page");
  const crumbs = page.getByRole("navigation", { name: "crumbs" });
  await expect(crumbs.getByRole("listitem")).toHaveText(["Overview", "Docs", "tour"]);
  await crumbs.getByRole("link", { name: "Docs" }).click();
  await expect(page).toHaveURL(/#docs$/);
  await expect(crumbs.getByRole("listitem")).toHaveText(["Overview", "Docs"]);
  // An admin has the host's pages; the host card says the version it runs.
  await expect(nav.getByRole("link", { name: "People and access" })).toBeVisible();
  await expect(nav.getByRole("region", { name: "host" })).toContainText(/holon \d+\.\d+\.\d+/);
  // The top bar opens the palette.
  await page.getByRole("button", { name: /Find or run/ }).click();
  await expect(page.getByRole("dialog", { name: "command palette" })).toBeVisible();
});

test("the command palette finds a page and a sample, and takes : commands", async ({ page }) => {
  await page.goto("/#library");
  await expect(navLink(page, "Library")).toHaveAttribute("aria-current", "page");
  const palette = page.getByRole("dialog", { name: "command palette" });
  const find = palette.getByRole("combobox");

  await page.keyboard.press("Control+k");
  await find.fill("flows");
  await expect(palette.getByRole("option").first()).toHaveText(/^Flows/);
  await page.keyboard.press("Enter");
  await expect(palette).toHaveCount(0);
  await expect(page).toHaveURL(/#flows$/);

  // A sample, by a few letters of its title: open in the editor, under its own name.
  await page.keyboard.press("/");
  await find.fill("new hello");
  await expect(palette.getByRole("option", { selected: true })).toContainText("New from Hello API");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#editor\.hello-api$/);

  // A line of its text: the text opens, there.
  await page.keyboard.press("Control+k");
  await find.fill(":2");
  await page.keyboard.press("Enter");
  await expect(page.locator(".cm-editor")).toBeVisible();
  await expect(page.locator(".cm-activeLine")).toHaveCount(1);

  // A command it doesn't know says so, and stays open; a theme is chosen, then auto again.
  await page.keyboard.press("Control+k");
  await find.fill(":frob");
  await page.keyboard.press("Enter");
  await expect(palette.getByRole("status")).toHaveText("there is no command :frob");
  await find.fill(":theme dark");
  await page.keyboard.press("Enter");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.keyboard.press("Control+k");
  await find.fill(":theme auto");
  await page.keyboard.press("Enter");
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", /./);
});

test("an address that names nothing says so, and shows the overview", async ({ page }) => {
  await page.goto("/#no.such.place");
  await expect(notices(page)).toContainText("Nothing is at #no.such.place");
  await expect(page).toHaveURL(/#overview$/);
});
