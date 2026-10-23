// Every sample, through the real editor and a real server (playwright.config.ts): opened from the
// flows page, deployed through the consent screen, and, for the ones that answer requests here,
// sent one and replayed exactly in view mode. What CLAUDE.md asks of a sample by hand ("run live
// once"), kept from rotting by CI. A sample deploys on its own port plus 1000 (set under ⚙), so a
// developer's deployments on the samples' ports are left alone.

import { expect, test, type Page } from "@playwright/test";

/** Open the sample titled `title` from the flows page: an unsaved copy in a tab of its own. */
async function openSample(page: Page, title: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const card = page.getByText(title, { exact: true }).locator("xpath=../..");
  await card.getByRole("button", { name: "open" }).click();
  await expect(page.locator(".hl-tab--active")).toBeVisible();
}

/** Deploy the open flow through ⚙ and the consent screen; its port moved up by 1000. Answers
 *  the port it serves on (null for a flow without routes). */
async function deploy(page: Page): Promise<number | null> {
  await page.locator(".hl-canvas").getByRole("button", { name: "⚙", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "deploy settings" });
  const portField = settings.locator("input").first();
  const given = Number(await portField.inputValue());
  const port = Number.isInteger(given) && given > 0 ? given + 1000 : null;
  if (port) await portField.fill(String(port));
  await settings.getByRole("button", { name: "done" }).click();
  await page.locator(".hl-canvas").getByRole("button", { name: /^(deploy|update)/ }).click();
  await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy|Update)$/ }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("deployed");
  return port;
}

/** View mode on the active tab, with the newest run selected (the log follows it live). */
async function viewNewestRun(page: Page) {
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).click();
  await expect(page.locator(".hl-view__bottom")).toContainText("replay reproduces the run");
}

test("every sample opens from the flows page and deploys", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  // The samples, as the flows page lists them.
  const titles = await page.locator("text=samples").locator("xpath=following-sibling::div").locator("div > div:first-child").allInnerTexts();
  const samples = titles.filter((t) => t.trim().length > 0);
  expect(samples.length).toBeGreaterThan(5);
  for (const title of samples) {
    await test.step(title, async () => {
      await openSample(page, title);
      await deploy(page);
    });
  }
});

test("hello-api answers, and its run replays exactly in view mode", async ({ page, request }) => {
  await openSample(page, "Hello API");
  const port = await deploy(page);
  const answer = await request.get(`http://127.0.0.1:${port}/hello/Ada`);
  expect(answer.status()).toBe(200);
  expect(await answer.text()).toContain("Hello, Ada");
  await viewNewestRun(page);
  // View mode is read only: no palette.
  await expect(page.locator(".hl-palette")).toHaveCount(0);
  await page.locator(".hl-canvas").getByRole("button", { name: "edit", exact: true }).click();
  await expect(page.locator(".hl-palette")).toBeVisible();
});

test("slugs runs its holons, and view mode shows inside one", async ({ page, request }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const port = await deploy(page);
  const answer = await request.post(`http://127.0.0.1:${port}/slugs`, { data: { title: "Hello Holon World", author: "Ada Lovelace" } });
  expect(await answer.json()).toMatchObject({ title: "hello-holon-world", author: "ada-lovelace", receipt: "unsigned" });
  await viewNewestRun(page);
  await page.locator(".react-flow__node", { hasText: "publish" }).click();
  await page.getByRole("button", { name: "Component", exact: true }).click();
  const side = page.locator(".hl-view__side");
  await expect(side).toContainText("inside, as the run went");
  await expect(side).toContainText('collection_put "posts"');
  await expect(side).toContainText("hello-holon-world");
});

test("a failing run replays exactly and opens on where it failed", async ({ page, request }) => {
  await openSample(page, "Hello API");
  const port = await deploy(page);
  // No such route: answered 404 by the server itself, not a run. A missing name is the flow's.
  expect((await request.get(`http://127.0.0.1:${port}/hello/`)).status()).toBe(404);
  await openSample(page, "A flow inside a flow (holon)");
  const slugs = await deploy(page);
  expect((await request.post(`http://127.0.0.1:${slugs}/slugs`, { data: { title: "only a title" } })).status()).toBe(400);
  await viewNewestRun(page);
  await expect(page.locator(".hl-view__bottom")).toContainText("failed at field author");
});

test("a holon expands into a canvas of its own, and ungroups in place", async ({ page }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  await flow.locator(".react-flow__node", { hasText: "publish" }).click();
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "expand" }).click();
  const overlay = page.locator(".hl-overlay");
  await expect(overlay.locator(".hl-overlay__crumbs")).toContainText("publish");
  await expect(overlay.locator(".react-flow__node", { hasText: "collection_put" })).toBeVisible();
  // Its boundary says what it is wired to in the flow around it.
  await expect(overlay.locator(".react-flow__node", { hasText: "inputs" })).toContainText("← slugify");
  await page.keyboard.press("Escape");
  await expect(overlay).toHaveCount(0);
  // Ungroup replaces it with its nodes, here.
  await flow.locator(".react-flow__node", { hasText: "publish" }).click();
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "ungroup" }).click();
  await expect(flow.locator(".react-flow__node", { hasText: "publish" })).toHaveCount(0);
  await expect(flow.locator(".react-flow__node", { hasText: "collection_put" })).toHaveCount(1);
});
