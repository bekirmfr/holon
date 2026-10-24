// Every sample, through the real editor and a real server (playwright.config.ts): opened from the
// flows page, deployed through the consent screen, and, for the ones that answer requests here,
// sent one and replayed exactly in view mode. What CLAUDE.md asks of a sample by hand ("run live
// once"), kept from rotting by CI. A sample deploys on its own port plus 1000 (set under ⚙), so a
// developer's deployments on the samples' ports are left alone.

import type { Page } from "@playwright/test";
import { expect, SERVER, test } from "./fixtures";

/** Open the sample titled `title` from the flows page: an unsaved copy in a tab of its own. */
async function openSample(page: Page, title: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const card = page.getByText(title, { exact: true }).locator("xpath=../..");
  await card.getByRole("button", { name: "open" }).click();
  await expect(page.locator(".hl-tab--active")).toBeVisible();
}

/** Deploy the open flow through ⚙ and the consent screen; its port moved up by `by` (1000).
 *  Answers the port it serves on (null for a flow without routes). */
async function deploy(page: Page, by = 1000): Promise<number | null> {
  await page.locator(".hl-canvas").getByRole("button", { name: "⚙", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "deploy settings" });
  const portField = settings.locator("input").first();
  const given = Number(await portField.inputValue());
  const port = Number.isInteger(given) && given > 0 ? given + by : null;
  if (port) await portField.fill(String(port));
  await settings.getByRole("button", { name: "done" }).click();
  await page.locator(".hl-canvas").getByRole("button", { name: /^(deploy|update)/ }).click();
  await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy|Update)$/ }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("deployed");
  return port;
}

/** Drop the palette entry `entry` (a primitive's name, or `flow:<name>` for a saved flow) on the
 *  canvas, as dragging it from the palette does: with the palette's own drag data, so it can't
 *  pick up a neighbouring row. */
async function dropFromPalette(page: Page, entry: string) {
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

test("a deployment whose port another program holds says, in view mode, it isn't answering", async ({ page, request }) => {
  const { createServer } = await import("node:net");
  // Hello API's port moved up by 2000: one no other test deploys on.
  const holder = createServer();
  await new Promise<void>((done) => holder.listen(8101 + 2000, "127.0.0.1", done));
  await openSample(page, "Hello API");
  const port = await deploy(page, 2000);
  expect(port).toBe(8101 + 2000);
  await expect(page.locator(".hl-canvas")).toContainText("port in use");
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).click();
  const banner = page.locator(".hl-view__not-listening");
  await expect(banner).toContainText(`could not listen on port ${port}`);
  await page.locator(".hl-canvas").getByRole("button", { name: "edit", exact: true }).click();

  // The port freed and deployed again: it answers there, and says nothing.
  await new Promise((done) => holder.close(done));
  expect(await deploy(page, 0)).toBe(port);
  await expect(page.locator(".hl-canvas")).not.toContainText("port in use");
  expect((await request.get(`http://127.0.0.1:${port}/hello/Ada`)).status()).toBe(200);
  await viewNewestRun(page);
  await expect(banner).toHaveCount(0);
});

test("the Inspector shows the selected node in edit mode: its ports, and its unit inside", async ({ page }) => {
  await openSample(page, "Hello API");
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(inspector).toContainText("select a node on the canvas");
  await page.locator(".react-flow__node", { hasText: "concat" }).click();
  await expect(inspector).toContainText("concat");
  await expect(inspector).toContainText("inputs");
  await inspector.getByRole("button", { name: "Unit", exact: true }).click();
  await expect(inspector).toContainText("Pure");
  // Folded to a strip, and opened again.
  await inspector.getByRole("button", { name: "fold the Inspector" }).click();
  await page.getByRole("button", { name: "open the Inspector" }).click();
  await expect(inspector).toContainText("Pure");
});

test("slugs runs its holons, and view mode shows inside one", async ({ page, request }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const port = await deploy(page);
  const answer = await request.post(`http://127.0.0.1:${port}/slugs`, { data: { title: "Hello Holon World", author: "Ada Lovelace" } });
  expect(await answer.json()).toMatchObject({ title: "hello-holon-world", author: "ada-lovelace", receipt: "unsigned" });
  await viewNewestRun(page);
  await page.locator(".react-flow__node", { hasText: "publish" }).click();
  const side = page.getByRole("complementary", { name: "Inspector" });
  await side.getByRole("button", { name: "Node", exact: true }).click();
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

test("an edited holon applies as a new version of itself, and the flow runs it", async ({ page, request }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  await flow.locator(".react-flow__node", { hasText: "publish" }).click();
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "expand" }).click();
  const overlay = page.locator(".hl-overlay");
  // The fallback receipt, changed inside.
  const receipt = overlay.locator('input[value="unsigned"]');
  await receipt.fill("not signed");
  await page.keyboard.press("Tab");
  await expect(overlay.locator(".hl-overlay__crumbs")).toContainText("publish •");
  // Undo inside is the level's own: the change goes, and comes back.
  await overlay.locator(".react-flow__pane").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("Control+z");
  await expect(overlay.locator(".hl-overlay__crumbs")).not.toContainText("•");
  await expect(overlay.locator('input[value="unsigned"]')).toHaveCount(1);
  await overlay.getByRole("button", { name: "redo" }).click();
  await expect(overlay.locator(".hl-overlay__crumbs")).toContainText("publish •");
  await overlay.getByRole("button", { name: "apply" }).click();
  await expect(overlay).toContainText("applied");
  await overlay.getByRole("button", { name: "close" }).click();
  await expect(overlay).toHaveCount(0);
  await expect(page.locator(".hl-canvas")).toContainText("is a new version");
  const port = await deploy(page);
  const answer = await request.post(`http://127.0.0.1:${port}/slugs`, { data: { title: "Edited Inside", author: "Ada" } });
  expect(await answer.json()).toMatchObject({ title: "edited-inside", receipt: "not signed" });
});

test("a port added inside a holon takes its first wire's type, and the flow's node gains it", async ({ page }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  await flow.locator(".react-flow__node", { hasText: "publish" }).click();
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "expand" }).click();
  const overlay = page.locator(".hl-overlay");
  await overlay.getByRole("button", { name: "add an output" }).click();
  await page.locator('[aria-modal="true"] input').fill("stamp");
  await page.keyboard.press("Enter");
  const outputs = overlay.locator(".react-flow__node", { hasText: "outputs" });
  const stamp = outputs.locator(".hl-port__label", { hasText: "stamp" });
  await expect(stamp).toHaveAttribute("title", /^any/);
  // Wired from the clock: it's a number now.
  const clockOut = overlay.locator(".react-flow__node", { hasText: "clock" }).locator(".react-flow__handle.source");
  await clockOut.dragTo(outputs.locator('.react-flow__handle[data-handleid="stamp"]'));
  await expect(stamp).toHaveAttribute("title", /^number/);
  await overlay.getByRole("button", { name: "apply" }).click();
  await expect(overlay).toContainText("applied");
  await overlay.getByRole("button", { name: "close" }).click();
  await expect(flow.locator(".react-flow__node", { hasText: "publish" })).toContainText("stamp");
});

test("the flows page says which saved flows place a flow as a node", async ({ page }) => {
  await page.goto("/");
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  // Names of their own, so a run never meets an earlier one's flows.
  const run = Date.now().toString(36);
  const [shout, user] = [`shout-${run}`, `loud-user-${run}`];
  /** A new flow named `name`, `entry` dropped from the palette onto it, saved. */
  const saveWith = async (name: string, entry: string) => {
    await page.getByRole("button", { name: "new", exact: true }).click();
    await page.locator('[aria-modal="true"] input').fill(name);
    await page.keyboard.press("Enter");
    await expect(page.locator(".hl-tab--active")).toContainText(name);
    await dropFromPalette(page, entry);
    await expect(flow.locator(".react-flow__node")).toHaveCount(1);
    await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
    await expect(page.locator(".hl-canvas")).toContainText("saved");
  };
  await saveWith(shout, "uppercase");
  await saveWith(user, `flow:${shout}`);
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await expect(page.getByText(`placed in ${user}`, { exact: true })).toBeVisible();
  // A save of "shout" that takes its input away asks first, naming who places it.
  await page.locator(".hl-tab", { hasText: shout }).click();
  await expect(page.locator(".hl-tab--active")).toContainText(shout);
  await expect(flow.locator(".react-flow__node", { hasText: "uppercase" })).toHaveCount(1);
  await flow.locator(".react-flow__node").click();
  await page.keyboard.press("Delete");
  await expect(flow.locator(".react-flow__node")).toHaveCount(0);
  await dropFromPalette(page, "concat");
  await expect(flow.locator(".react-flow__node", { hasText: "concat" })).toHaveCount(1);
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  const ask = page.locator('[aria-modal="true"]');
  await expect(ask).toContainText(`"${shout}" is placed as a node in ${user}`);
  await expect(ask).toContainText('input "uppercase · s" removed');
  await ask.getByRole("button", { name: "Save" }).click();
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await expect(page.getByText(`placed in ${user} (an older save)`, { exact: true })).toBeVisible();
});

test("the marketplace, a Holon flow: publish a saved flow to it, place it from the palette", async ({ page, request, adminToken }) => {
  await openSample(page, "Marketplace (built with Holon)");
  const port = await deploy(page);
  const market = `http://127.0.0.1:${port}`;
  const key = "e2e-publish-key-0123456789";
  const asAdmin = { Authorization: `Bearer ${adminToken}` };
  expect((await request.post(`${SERVER}/deployments/marketplace/secrets/publish_key`, { data: { value: key }, headers: asAdmin })).ok()).toBe(true);
  // Without the key, nothing is published.
  expect((await request.post(`${market}/listings`, { data: { name: "x", root: "y" } })).status()).toBe(401);

  const run = Date.now().toString(36);
  const shout = `shout-mk-${run}`;
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  const ask = page.locator('[aria-modal="true"]');
  const answer = async (text: string) => {
    await ask.locator("input").fill(text);
    await page.keyboard.press("Enter");
  };
  await page.getByRole("button", { name: "new", exact: true }).click();
  await answer(shout);
  await expect(page.locator(".hl-tab--active")).toContainText(shout);
  await dropFromPalette(page, "uppercase");
  await expect(flow.locator(".react-flow__node")).toHaveCount(1);
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await expect(page.locator(".hl-canvas")).toContainText("saved");

  // Set the marketplace, and publish from the flows page.
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await page.getByRole("button", { name: /^(set|change)$/ }).click();
  await answer(market);
  await expect(page.getByText(`publishing to and placing from ${market}`)).toBeVisible();
  await page.getByText(shout, { exact: true }).locator("xpath=..").getByRole("button", { name: "publish" }).click();
  await answer("shouts its text");
  await answer("e2e");
  await answer(key);
  await expect(page.getByText(`published "${shout}"`)).toBeVisible();
  const listings = (await (await request.get(`${market}/listings`)).json()) as Record<string, { id: string; name: string }>;
  const listed = Object.values(listings).find((l) => l.name === shout)!;

  // In another flow, from the palette's marketplace: seen what it may do, placed.
  await page.getByRole("button", { name: "new", exact: true }).click();
  await answer(`uses-mk-${run}`);
  await expect(page.locator(".hl-palette")).toContainText(shout);
  await dropFromPalette(page, `market:${listed.id}`);
  await expect(ask).toContainText("It only computes");
  await ask.getByRole("button", { name: "Place" }).click();
  await expect(flow.locator(".react-flow__node", { hasText: shout })).toHaveCount(1);

  // Yanked from the flows page: flagged, never edited, and the palette says so.
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const row = page.locator("div", { has: page.getByText(shout, { exact: true }) }).filter({ hasText: "by e2e" }).last();
  await row.getByRole("button", { name: "yank" }).click();
  await ask.getByRole("button", { name: "Yank" }).click();
  await expect(row).toContainText("yanked");
  const after = (await (await request.get(`${market}/listings/${encodeURIComponent(listed.id)}`)).json()) as { yanked?: boolean; units: string[] };
  expect(after.yanked).toBe(true);
  expect(after.units.length).toBeGreaterThan(0);
});

test("a flow as text: exported as .holon, checked by the command line, imported back", async ({ page }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const name = `texty-${Date.now().toString(36)}`;
  await page.goto("/");
  await page.getByRole("button", { name: "new", exact: true }).click();
  await page.locator('[aria-modal="true"] input').fill(name);
  await page.keyboard.press("Enter");
  await expect(page.locator(".hl-tab--active")).toContainText(name);
  await dropFromPalette(page, "uppercase");
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await expect(page.locator(".hl-canvas")).toContainText("saved");

  // Exported from the flows page.
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const row = page.getByText(name, { exact: true }).locator("xpath=..");
  const download = page.waitForEvent("download");
  await row.getByRole("button", { name: ".holon" }).click();
  const file = join(mkdtempSync(join(tmpdir(), "holon-text-")), `${name}.holon`);
  await (await download).saveAs(file);
  const text = readFileSync(file, "utf8");
  expect(text).toContain(`flow ${name}`);
  expect(text).toMatch(/node uppercase = uppercase/);

  // Checked on the command line, with no browser: the `holon` the e2e server was built as.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const checked = execFileSync(holon, ["check", file], { encoding: "utf8" });
  expect(checked).toContain(`${name}: 1 nodes, 0 wires`);
  expect(checked).toContain("it only computes");

  // Imported back: an unsaved tab of its own, with the same node.
  await page.getByLabel("import a .holon file").setInputFiles(file);
  await expect(page.locator(".hl-tab--active")).toContainText(name);
  await expect(page.locator(".hl-canvas__flow > .react-flow .react-flow__node", { hasText: "uppercase" })).toHaveCount(1);
});
