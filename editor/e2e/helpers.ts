// What the tests do through the editor's shell, in one place: going to a page, opening a sample,
// deploying through the consent screen, viewing runs. When the shell changes (its nav, its
// pages), these change, and the tests that use them don't.

import type { Locator, Page } from "@playwright/test";
import { expect, SAMPLE_SHIFT } from "./fixtures";

/** The pages the side nav goes to (ui/Shell.tsx), and "new" for its new flow. */
export type MenuPage = "flows" | "library" | "docs" | "admin" | "new";

/** The nav's entry for `name`: admin's pages are under Resources until they have their own. */
export function menuEntry(page: Page, name: MenuPage): Locator {
  const nav = page.getByRole("navigation", { name: "main" });
  if (name === "new") return nav.getByRole("button", { name: "new flow" });
  const label = { flows: "Flows", library: "Library", docs: "Docs", admin: "Resources" }[name];
  return nav.getByRole("link", { name: label, exact: true });
}

/** The side nav's entry `label` ("Flows", "Getting started"…): the page shown is its
 *  `aria-current`. */
export function navLink(page: Page, label: string): Locator {
  return page.getByRole("navigation", { name: "main" }).getByRole("link", { name: label, exact: true });
}

/** Go to `name` from the side nav, as a person would. */
export async function gotoPage(page: Page, name: MenuPage) {
  await menuEntry(page, name).click();
}

/** Open the sample titled `title` from the Library's samples: an unsaved copy in a tab of its
 *  own. From a fresh load of the editor unless `reload` is false: then from the Library of the
 *  editor as it is (faster, and its other tabs stay open). */
export async function openSample(page: Page, title: string, reload = true) {
  if (reload) await page.goto("/#library.samples");
  else await gotoPage(page, "library");
  await page.getByRole("region", { name: "samples" }).getByRole("button", { name: `open ${title}`, exact: true }).click();
  await expect(page.locator(".hl-tab--active")).toBeVisible();
}

/** The Flows page's row of the flow `name`, saved or deployed. */
export function flowRow(page: Page, name: string): Locator {
  return page.getByRole("region", { name: "flows" }).getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) });
}

/** Wait until `locator` stops moving: its box the same over two reads 100 ms apart. An overlay
 *  fits its view again a moment after it opens (CompositeOverlay.tsx, 240 ms then 200 ms of
 *  animation), and a drag begun meanwhile aims at where a handle a few pixels wide was, and
 *  misses it. */
export async function still(locator: Locator) {
  let last = "";
  await expect
    .poll(
      async () => {
        const now = JSON.stringify(await locator.boundingBox());
        const same = now === last;
        last = now;
        return same;
      },
      { intervals: [100], timeout: 5_000 },
    )
    .toBe(true);
}

/** Open the deployment settings panel if it is closed. */
export async function openDeployPanel(page: Page) {
  const panel = page.getByRole("complementary", { name: "Deployment settings" });
  if (!(await panel.isVisible())) {
    await page.getByRole("button", { name: "open deployment settings" }).click();
    await panel.waitFor({ state: "visible", timeout: 5_000 });
  }
}

/** Deploy the open flow through the deployment settings panel and the consent screen; its port
 *  moved up by `by` (the slot's SAMPLE_SHIFT, ports.mjs), and whatever else `set` sets in the
 *  panel. Answers the port it serves on (null for a flow without routes). */
export async function deploy(page: Page, by = SAMPLE_SHIFT, set?: (settings: Locator) => Promise<void>): Promise<number | null> {
  await openDeployPanel(page);
  const settings = page.getByRole("complementary", { name: "Deployment settings" });
  const portField = settings.getByRole("textbox").first();
  const given = Number(await portField.inputValue());
  const port = Number.isInteger(given) && given > 0 ? given + by : null;
  if (port) await portField.fill(String(port));
  if (set) await set(settings);
  const deployBtn = page.locator(".hl-canvas").getByRole("button", { name: /^(deploy|update)/ });
  await expect(deployBtn).toBeEnabled();
  await deployBtn.dispatchEvent("click");
  // Wait for the server's answer to the deploy, which comes once the flow listens on its port
  // (http.rs syncs the listeners first). The tab's "deployed" can't say so: it shows as soon as a
  // deployment of that name exists, so on a redeploy that moves the port it is there at once.
  const deployed = page.waitForResponse((r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy");
  await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy|Update)$/ }).click();
  expect((await deployed).status()).toBe(200);
  await expect(page.locator(".hl-tab--active")).toContainText("deployed", { timeout: 30_000 });
  return port;
}

/** Wait until text typed into the Text pane has compiled: the deployment settings panel then says
 *  its port. */
export async function settingsSayPort(page: Page, port: string) {
  await openDeployPanel(page);
  const settings = page.getByRole("complementary", { name: "Deployment settings" });
  await expect(settings.getByRole("textbox").first()).toHaveValue(port, { timeout: 15_000 });
}

/** Drop the palette entry `entry` (a primitive's name, or `flow:<name>` for a saved flow) on the
 *  canvas, as dragging it from the palette does: with the palette's own drag data, so it can't
 *  pick up a neighbouring row. */
export async function dropFromPalette(page: Page, entry: string) {
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  const box = (await flow.boundingBox())!;
  const dataTransfer = await page.evaluateHandle((name) => {
    const d = new DataTransfer();
    d.setData("application/holon-primitive", name);
    return d;
  }, entry);
  await flow.dispatchEvent("drop", { dataTransfer, clientX: box.x + 300, clientY: box.y + 200 });
}

/** View mode on the active tab, with the newest run selected (the log follows it live). */
export async function viewNewestRun(page: Page) {
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).dispatchEvent("click");
  await expect(page.locator(".hl-view__bottom")).toContainText("replay reproduces the run");
}

/** What the editor says once, in the corner (ui/Toast.tsx): saved, updated, not saved. */
export function notices(page: Page): Locator {
  return page.getByRole("region", { name: "notices" });
}
