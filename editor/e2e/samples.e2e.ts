// Every sample, through the real editor and a real server (playwright.config.ts): opened from the
// flows page, deployed through the consent screen, and, for the ones that answer requests here,
// sent one and replayed exactly in view mode. What CLAUDE.md asks of a sample by hand ("run live
// once"), kept from rotting by CI. A sample deploys on its own port plus 1000 (set under ⚙), so a
// developer's deployments on the samples' ports are left alone, plus 100 for each slot past the
// first (ports.mjs), so runs side by side don't meet.

import { appendFileSync, readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { expect, FOLDERS, PORTS, RUN_DIR, SAMPLE_SHIFT, SERVER, test } from "./fixtures";

const ATLAS_NOTES = "Atlas notes (a MongoDB database)";

/** Open the sample titled `title` from the flows page: an unsaved copy in a tab of its own. From
 *  a fresh load of the editor unless `reload` is false: then from the flows page of the editor
 *  as it is (faster, and its other tabs stay open). */
async function openSample(page: Page, title: string, reload = true) {
  if (reload) await page.goto("/");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const card = page.getByText(title, { exact: true }).locator("xpath=../..");
  await card.getByRole("button", { name: "open" }).click();
  await expect(page.locator(".hl-tab--active")).toBeVisible();
}

/** Deploy the open flow through ⚙ and the consent screen; its port moved up by `by` (the slot's
 *  SAMPLE_SHIFT, ports.mjs), and
 *  whatever else `set` sets under ⚙. Answers the port it serves on (null for a flow without routes). */
async function deploy(page: Page, by = SAMPLE_SHIFT, set?: (settings: Locator) => Promise<void>): Promise<number | null> {
  await page.locator(".hl-canvas").getByRole("button", { name: "⚙", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "deploy settings" });
  const portField = settings.locator("input").first();
  const given = Number(await portField.inputValue());
  const port = Number.isInteger(given) && given > 0 ? given + by : null;
  if (port) await portField.fill(String(port));
  if (set) await set(settings);
  await settings.getByRole("button", { name: "done" }).click();
  await page.locator(".hl-canvas").getByRole("button", { name: /^(deploy|update)/ }).click();
  // Wait for the server's answer to the deploy, which comes once the flow listens on its port
  // (http.rs syncs the listeners first). The tab's "deployed" can't say so: it shows as soon as a
  // deployment of that name exists, so on a redeploy that moves the port it is there at once.
  const deployed = page.waitForResponse((r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy");
  await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy|Update)$/ }).click();
  expect((await deployed).status()).toBe(200);
  await expect(page.locator(".hl-tab--active")).toContainText("deployed");
  return port;
}

/** Wait until text typed into the Text pane has compiled: ⚙ then says its port. Opened again if a
 *  compile that lands after the click rebuilds the canvas and closes it. */
async function settingsSayPort(page: Page, port: string) {
  const settings = page.getByRole("dialog", { name: "deploy settings" });
  await expect(async () => {
    if (!(await settings.isVisible())) await page.locator(".hl-canvas").getByRole("button", { name: "⚙", exact: true }).click();
    await expect(settings.locator("input").first()).toHaveValue(port, { timeout: 1_000 });
  }).toPass({ timeout: 15_000 });
  await settings.getByRole("button", { name: "done" }).click();
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

/** The samples' titles, as holon-author lists them (samples.rs), read as the tests are collected:
 *  a test for each group of SAMPLES_PER_TEST below, so they spread over shards (ci.yml) rather
 *  than make one long test, without a fresh page and editor for each sample (a test each was
 *  half again as slow). */
const SAMPLE_TITLES = [...readFileSync(new URL("../../crates/holon-author/src/samples.rs", import.meta.url), "utf8").matchAll(/^\s*title: "(.*)",$/gm)].map((m) => m[1]);

test("the flows page lists every sample", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const titles = await page.locator("text=samples").locator("xpath=following-sibling::div").locator("div > div:first-child").allInnerTexts();
  const listed = titles.filter((t) => t.trim().length > 0);
  // Every sample samples.rs has, so none is left out of the tests below, and nothing else.
  expect(SAMPLE_TITLES.length).toBeGreaterThan(5);
  expect([...listed].sort()).toEqual([...SAMPLE_TITLES].sort());
});

const SAMPLES_PER_TEST = 8;
for (let first = 0; first < SAMPLE_TITLES.length; first += SAMPLES_PER_TEST) {
  const titles = SAMPLE_TITLES.slice(first, first + SAMPLES_PER_TEST);
  test(`samples ${first + 1} to ${first + titles.length} open from the flows page and deploy`, async ({ page }) => {
    await page.goto("/");
    for (const title of titles) {
      // Its database is a MongoDB, bound only where the environment names one (CI does).
      if (title === ATLAS_NOTES && !process.env.HOLON_TEST_MONGODB) continue;
      await test.step(title, async () => {
        await openSample(page, title, false);
        await deploy(page);
      });
    }
  });
}

test("a sample deployed as it is keeps its text and the names it binds", async ({ page, request, adminToken }) => {
  // The port moved in the text (the text's wins while it is open), so the text still stands for
  // the canvas: a deploy then carries `source` and `bindings` (useDeployFlow.ts), and a run explains
  // as the text reads (`name = req.params.name`, not `field_2 = …`). Changing the port under ⚙
  // alone is a canvas change the text doesn't have, so the loop above deploys the canvas alone.
  await openSample(page, "Cache-aside (a branch not taken)");
  const port = 8115 + SAMPLE_SHIFT;
  const text = readFileSync(new URL("../../crates/holon-author/samples/cache-aside.holon", import.meta.url), "utf8");
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(text.replace("port 8115", `port ${port}`));
  await settingsSayPort(page, String(port));
  expect(await deploy(page, 0)).toBe(port);
  const deployed = await request.get(`${SERVER}/deployments/cache-aside/document`, { headers: { Authorization: `Bearer ${adminToken}` } });
  expect(deployed.status()).toBe(200);
  const document = (await deployed.json()) as { source?: string; bindings?: Record<string, string> };
  expect(document.source).toContain("flow cache-aside");
  expect(Object.values(document.bindings ?? {})).toEqual(expect.arrayContaining(["name", "id"]));
});

test("hello-api answers, and its run replays exactly in view mode", async ({ page, request }) => {
  await openSample(page, "Hello API");
  const port = await deploy(page);
  const answer = await request.get(`http://127.0.0.1:${port}/hello/Ada`);
  expect(answer.status()).toBe(200);
  expect(await answer.text()).toContain("Hello, Ada");
  await viewNewestRun(page);
  // The bottom pane's top edge drags its height (ViewPanes.tsx), and the canvas above takes the rest.
  const pane = page.getByRole("region", { name: "run" });
  const before = (await pane.boundingBox())!.height;
  const edge = (await pane.locator(".hl-view__edge").boundingBox())!;
  await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2);
  await page.mouse.down();
  await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2 - 120, { steps: 6 });
  await page.mouse.up();
  expect((await pane.boundingBox())!.height).toBeGreaterThan(before + 80);
  // View mode is read only: no palette.
  await expect(page.locator(".hl-palette")).toHaveCount(0);
  await page.locator(".hl-canvas").getByRole("button", { name: "edit", exact: true }).click();
  await expect(page.locator(".hl-palette")).toBeVisible();
});

test("a deployment whose port another program holds says, in view mode, it isn't answering", async ({ page, request }) => {
  const { createServer } = await import("node:net");
  // Hello API's port moved up 1000 past where samples deploy: one no other test deploys on.
  const holder = createServer();
  await new Promise<void>((done) => holder.listen(8101 + SAMPLE_SHIFT + 1000, "127.0.0.1", done));
  await openSample(page, "Hello API");
  const port = await deploy(page, SAMPLE_SHIFT + 1000);
  expect(port).toBe(8101 + SAMPLE_SHIFT + 1000);
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
  // The node's line in the flow as text (docs/LANGUAGE.md): the whole respond, since concat is
  // part of that expression.
  await expect(inspector).toContainText("as text, line");
  await expect(inspector.locator(".hl-inspector__source")).toHaveText('respond "Hello, {req.params.name}"');
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
  const ask = page.locator('[aria-modal="true"]');
  /** A new flow, `entry` dropped from the palette onto it, saved as `name` (asked on save). */
  const saveWith = async (name: string, entry: string) => {
    await page.getByRole("button", { name: "new", exact: true }).click();
    await expect(page.locator(".hl-tab--active")).toContainText("untitled");
    await dropFromPalette(page, entry);
    await expect(flow.locator(".react-flow__node")).toHaveCount(1);
    await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
    await ask.locator("input").fill(name);
    await page.keyboard.press("Enter");
    await expect(page.locator(".hl-canvas")).toContainText("saved");
    await expect(page.locator(".hl-tab--active")).toContainText(name);
  };
  await saveWith(shout, "uppercase");
  await saveWith(user, `flow:${shout}`);
  // A draft named on save as a flow saved already asks before replacing it; cancelled, it stays untitled.
  await page.getByRole("button", { name: "new", exact: true }).click();
  await dropFromPalette(page, "concat");
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await ask.locator("input").fill(shout);
  await page.keyboard.press("Enter");
  await expect(ask).toContainText(`A flow named "${shout}" is saved already`);
  await ask.getByRole("button", { name: "Cancel" }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("untitled");
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
  await dropFromPalette(page, "uppercase");
  await expect(flow.locator(".react-flow__node")).toHaveCount(1);
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await answer(shout);
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
  await dropFromPalette(page, "uppercase");
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await page.locator('[aria-modal="true"] input').fill(name);
  await page.keyboard.press("Enter");
  await expect(page.locator(".hl-canvas")).toContainText("saved");
  await expect(page.locator(".hl-tab--active")).toContainText(name);

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

test("likes answers a count in a sentence: the number as text (as_text), then the words", async ({ page, request }) => {
  await openSample(page, "Likes counter (runs side by side)");
  const port = await deploy(page);
  // A fresh post: liked once, then shown.
  const post = `e2e-${Date.now()}`;
  expect((await request.post(`http://127.0.0.1:${port}/posts/${post}/like`)).status()).toBe(200);
  const shown = await request.get(`http://127.0.0.1:${port}/posts/${post}/likes`);
  expect(shown.status()).toBe(200);
  expect(await shown.text()).toBe("1 likes");
  // Never liked: the count falls back to 0, and as_text writes it.
  expect(await (await request.get(`http://127.0.0.1:${port}/posts/${post}-never/likes`)).text()).toBe("0 likes");
});

test("basket reads a price inside a mapped function, per item, and its run replays exactly", async ({ page, request }) => {
  await openSample(page, "Basket (a collection read inside a function)");
  const port = await deploy(page);
  const [tea, cake] = [`tea-${Date.now()}`, `cake-${Date.now()}`];
  expect((await request.put(`http://127.0.0.1:${port}/prices/${tea}`, { data: { price: 4 } })).status()).toBe(200);
  expect((await request.put(`http://127.0.0.1:${port}/prices/${cake}`, { data: { price: 3 } })).status()).toBe(200);
  const basket = await request.post(`http://127.0.0.1:${port}/basket`, { data: { items: [{ name: tea, qty: 2 }, { name: cake, qty: 1 }] } });
  expect(basket.status()).toBe(200);
  expect(await basket.json()).toEqual({ lines: [{ name: tea, total: 8 }, { name: cake, total: 3 }], total: 11 });
  // The map's reads aren't the canvas's nodes: view mode shows it with what the run recorded.
  await viewNewestRun(page);
  expect((await request.post(`http://127.0.0.1:${port}/basket`, { data: { items: [{ name: `scone-${Date.now()}`, qty: 1 }] } })).status()).toBe(404);
});

test("live-board's page gets each ticket posted as a pack on its channel, and a reconnect gets the ones it missed", async ({ page, browser, request }) => {
  await openSample(page, "Live board (a channel: packs to whoever listens)");
  const port = await deploy(page);
  // The board itself, subscribed (its EventSource on GET /live) before anything is posted.
  const board = await browser.newPage();
  const subscribed = board.waitForResponse((r) => new URL(r.url()).pathname === "/live");
  await board.goto(`http://127.0.0.1:${port}/`);
  expect((await subscribed).headers()["content-type"]).toBe("text/event-stream");
  const title = `e2e-${Date.now()}`;
  const posted = await request.post(`http://127.0.0.1:${port}/tickets`, { data: { title } });
  expect(posted.status()).toBe(201);
  await expect(board.locator("li.new")).toHaveText([title]);
  // Without a title nothing is stored or sent.
  expect((await request.post(`http://127.0.0.1:${port}/tickets`, { data: {} })).status()).toBe(400);
  // A subscriber that reconnects after the last pack it saw gets the kept ones past it, here as a
  // stream read for a moment.
  const events = await board.evaluate(
    async ([url, title]) => {
      const ctl = new AbortController();
      const r = await fetch(url, { headers: { "last-event-id": "0.0" }, signal: ctl.signal });
      const reader = r.body!.getReader();
      let text = "";
      while (!text.includes(title)) {
        const { value, done } = await reader.read();
        if (done) break;
        text += new TextDecoder().decode(value);
      }
      ctl.abort();
      return text;
    },
    [`http://127.0.0.1:${port}/live`, title],
  );
  expect(events).toContain(`"title":"${title}"`);
  await board.close();
});

test("approval sends an order's progress on its channel: waiting as the run parks, paid when it finishes, never the first twice", async ({ page, browser, request }) => {
  await openSample(page, "Order approval (durable run)");
  const port = await deploy(page);
  const item = `e2e-${Date.now()}`;
  const link = await (await request.post(`http://127.0.0.1:${port}/orders`, { data: { item } })).text();
  expect(link).toMatch(/^\/paid\//);
  // The channel's kept packs, read for a moment from a page of the flow's own (the stream stays
  // open, so read until `until`).
  const own = await browser.newPage();
  await own.goto(`http://127.0.0.1:${port}/nothing`);
  const live = (until: string) =>
    own.evaluate(
      async ([url, until]) => {
        const ctl = new AbortController();
        const r = await fetch(url, { signal: ctl.signal });
        const reader = r.body!.getReader();
        let text = "";
        while (!text.includes(until)) {
          const { value, done } = await reader.read();
          if (done) break;
          text += new TextDecoder().decode(value);
        }
        ctl.abort();
        return text;
      },
      [`http://127.0.0.1:${port}/orders/live`, until],
    );
  expect(await live(item)).toContain(`"item":"${item}","status":"waiting for payment"`);
  expect((await request.get(`http://127.0.0.1:${port}${link}`)).ok()).toBe(true);
  const after = await live(`"status":"paid"`);
  const mine = after.split("\n").filter((l) => l.startsWith("data:") && l.includes(item));
  expect(mine).toHaveLength(2);
  expect(mine[1]).toContain(`"status":"paid","via":"GET"`);
  await own.close();
});

test("chat-proxy forwards its upstream's answer as it arrives, and stores the whole of it once it ends", async ({ page, browser, request, adminToken }) => {
  const { createServer } = await import("node:http");
  const { readFileSync } = await import("node:fs");
  // A stand-in for the Messages API on loopback: it streams one piece, then holds the rest until
  // the test has seen the first on the page, so the page can only have it if it was forwarded.
  let release = () => {};
  const held = new Promise<void>((r) => (release = r));
  const asked: string[] = [];
  const event = (data: object) => `event: ${(data as { type: string }).type}
data: ${JSON.stringify(data)}

`;
  const upstream = createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      asked.push(`${req.headers["x-api-key"]} ${JSON.parse(body).messages[0].content}`);
      // A key it doesn't know is refused, as the API does: a 401 and why, not an event stream.
      if (req.headers["x-api-key"] === "sk-e2e-revoked") {
        res.writeHead(401, { "content-type": "application/json" });
        res.end(JSON.stringify({ type: "error", error: { type: "authentication_error", message: "invalid x-api-key" } }));
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.write(event({ type: "message_start", message: { id: "msg_e2e" } }));
      res.write(event({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Rivers run" } }));
      await held;
      res.write(event({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: " to the sea." } }));
      res.end(event({ type: "message_stop" }));
    });
  });
  await new Promise<void>((r) => upstream.listen(0, "127.0.0.1", r));
  const host = `127.0.0.1:${(upstream.address() as { port: number }).port}`;
  try {
    await openSample(page, "Chat proxy (a streamed reply)");
    // The sample calls api.anthropic.com: here, the stand-in, over plain HTTP to this machine. The
    // text says the port (with the text open, it is the text's), moved up as deploy() moves it.
    const text = readFileSync(new URL("../../crates/holon-author/samples/chat-proxy.holon", import.meta.url), "utf8");
    await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
    const editor = page.getByRole("region", { name: "Text" }).locator(".cm-content");
    await editor.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text.replaceAll('"api.anthropic.com"', `"${host}"`).replace("port 8127", `port ${8127 + SAMPLE_SHIFT}`));
    await settingsSayPort(page, String(8127 + SAMPLE_SHIFT));
    const port = await deploy(page, 0, async (settings) => {
      await settings.getByTitle("comma-separated host[:port]").fill(host);
      await settings.getByText("private addresses", { exact: true }).locator("xpath=following-sibling::input[1]").check();
    });
    const key = "sk-e2e-0123456789";
    expect((await request.post(`${SERVER}/deployments/chat-proxy/secrets/anthropic_api_key`, { data: { value: key }, headers: { Authorization: `Bearer ${adminToken}` } })).ok()).toBe(true);
    const chat = await browser.newPage();
    await chat.goto(`http://127.0.0.1:${port}/`);
    await chat.locator("input[name=prompt]").fill("a line about rivers");
    await chat.getByRole("button", { name: "Ask" }).click();
    await expect(chat.locator("#said")).toHaveText("Rivers run");
    release();
    await expect(chat.locator("#said")).toHaveText("Rivers run to the sea.");
    expect(asked).toEqual([`${key} a line about rivers`]);
    // Listed on the page as soon as the answer ends, with no reload.
    await expect(chat.locator("#past li").first()).toHaveText("a line about riversRivers run to the sea.");
    // The run had the whole answer: stored, and listed on the page.
    await expect(async () => {
      await chat.reload();
      await expect(chat.locator("li")).toContainText(["a line about riversRivers run to the sea."], { timeout: 1000 });
    }).toPass();
    // Without a prompt nothing is called.
    expect((await request.post(`http://127.0.0.1:${port}/chat`, { data: {} })).status()).toBe(400);
    expect(asked).toHaveLength(1);
    // The room: a piped call's pieces to whoever watches, as they arrive, then the run's pack.
    await chat.evaluate(() => {
      const w = window as unknown as { heard: string[] };
      w.heard = [];
      new EventSource("/room/live").onmessage = (e) => w.heard.push(e.data);
    });
    await expect.poll(() => request.get(`${SERVER}/deployments/chat-proxy/log`, { headers: { Authorization: `Bearer ${adminToken}` } }).then((r) => r.text())).toContain("GET /room/live");
    const roomed = await request.post(`http://127.0.0.1:${port}/room`, { data: { prompt: "the room" } });
    expect(await roomed.json()).toEqual({ prompt: "the room", said: "Rivers run to the sea." });
    const heard = () => chat.evaluate(() => (window as unknown as { heard: string[] }).heard);
    await expect.poll(heard).toContainEqual(JSON.stringify({ prompt: "the room", said: "Rivers run to the sea." }));
    expect((await heard()).slice(0, -1).map((piece) => JSON.parse(piece)).join("")).toContain('"text":"Rivers run"');
    // A key the API refuses: nothing is forwarded, and the reply is the refusal with its status,
    // which the page shows.
    expect((await request.post(`${SERVER}/deployments/chat-proxy/secrets/anthropic_api_key`, { data: { value: "sk-e2e-revoked" }, headers: { Authorization: `Bearer ${adminToken}` } })).ok()).toBe(true);
    await chat.reload();
    await chat.locator("input[name=prompt]").fill("refused");
    await chat.getByRole("button", { name: "Ask" }).click();
    await expect(chat.locator("#said")).toContainText("the model's API answered 401");
    await expect(chat.locator("#said")).toContainText("invalid x-api-key");
    expect((await request.post(`http://127.0.0.1:${port}/chat`, { data: { prompt: "refused" } })).status()).toBe(401);
    await chat.close();
  } finally {
    release();
    upstream.close();
  }
});

test("gemini-chat forwards Gemini's streamed answer as it arrives, and says so when the API refuses", async ({ page, browser, request, adminToken }) => {
  const { createServer } = await import("node:http");
  const { readFileSync } = await import("node:fs");
  // A stand-in for the Interactions API on loopback: step.delta events, then [DONE]; a key it
  // doesn't know is refused, as the API does.
  const asked: string[] = [];
  const event = (data: { event_type: string }) => `event: ${data.event_type}\ndata: ${JSON.stringify(data)}\n\n`;
  const upstream = createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const sent = JSON.parse(body);
      asked.push(`${req.headers["x-goog-api-key"]} ${sent.model} ${sent.input}`);
      if (req.headers["x-goog-api-key"] === "e2e-revoked") {
        res.writeHead(429, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: { code: 429, message: "Resource has been exhausted (e.g. check quota).", status: "RESOURCE_EXHAUSTED" } }));
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.write(event({ event_type: "interaction.created" }));
      res.write(event({ event_type: "step.delta", index: 1, delta: { type: "text", text: "Rivers run" } } as { event_type: string }));
      res.write(event({ event_type: "step.delta", index: 1, delta: { type: "text", text: " to the sea." } } as { event_type: string }));
      res.end(event({ event_type: "interaction.completed" }) + "data: [DONE]\n\n");
    });
  });
  await new Promise<void>((r) => upstream.listen(0, "127.0.0.1", r));
  const host = `127.0.0.1:${(upstream.address() as { port: number }).port}`;
  try {
    await openSample(page, "Gemini chat (a streamed reply, free tier)");
    // The sample calls generativelanguage.googleapis.com: here, the stand-in, over plain HTTP to
    // this machine, on a port the text says (the text's wins while it is open).
    const text = readFileSync(new URL("../../crates/holon-author/samples/gemini-chat.holon", import.meta.url), "utf8");
    await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
    const editor = page.getByRole("region", { name: "Text" }).locator(".cm-content");
    await editor.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text.replaceAll('"generativelanguage.googleapis.com"', `"${host}"`).replace("port 8128", `port ${8128 + SAMPLE_SHIFT}`));
    await settingsSayPort(page, String(8128 + SAMPLE_SHIFT));
    const port = await deploy(page, 0, async (settings) => {
      await settings.getByTitle("comma-separated host[:port]").fill(host);
      await settings.getByText("private addresses", { exact: true }).locator("xpath=following-sibling::input[1]").check();
    });
    const secret = (value: string) => request.post(`${SERVER}/deployments/gemini-chat/secrets/gemini_api_key`, { data: { value }, headers: { Authorization: `Bearer ${adminToken}` } });
    expect((await secret("e2e-key")).ok()).toBe(true);
    const chat = await browser.newPage();
    await chat.goto(`http://127.0.0.1:${port}/`);
    await chat.locator("input[name=prompt]").fill("a line about rivers");
    await chat.getByRole("button", { name: "Ask" }).click();
    await expect(chat.locator("#said")).toHaveText("Rivers run to the sea.");
    expect(asked).toEqual(["e2e-key gemini-3.5-flash-lite a line about rivers"]);
    // Listed on the page as soon as the answer ends, with no reload.
    await expect(chat.locator("#past li").first()).toHaveText("a line about riversRivers run to the sea.");
    // The run had the whole answer: stored, and listed on the page.
    await expect(async () => {
      await chat.reload();
      await expect(chat.locator("li")).toContainText(["a line about riversRivers run to the sea."], { timeout: 1000 });
    }).toPass();
    // The free tier's limit: nothing forwarded, the refusal with its status, which the page shows.
    expect((await secret("e2e-revoked")).ok()).toBe(true);
    await chat.locator("input[name=prompt]").fill("refused");
    await chat.getByRole("button", { name: "Ask" }).click();
    await expect(chat.locator("#said")).toContainText("Gemini answered 429");
    await expect(chat.locator("#said")).toContainText("RESOURCE_EXHAUSTED");
    await chat.close();
  } finally {
    upstream.close();
  }
});

test("cache-aside answers a name it has from its collection without the fetch, which view mode shows as not run", async ({ page, request }) => {
  await openSample(page, "Cache-aside (a branch not taken)");
  const port = await deploy(page);
  const name = `ada-${Date.now()}`;
  // Set its id (so the test needs no network), then ask for it: the ?? has it, so the fetch, its
  // fallback, never runs.
  expect((await request.put(`http://127.0.0.1:${port}/id/${name}`, { data: { id: "x-1" } })).status()).toBe(200);
  const got = await request.get(`http://127.0.0.1:${port}/id/${name}`);
  expect(got.status()).toBe(200);
  expect(await got.text()).toContain("x-1");
  // The replay takes the branch the run took, which the run's timeline confirms.
  await viewNewestRun(page);
  const skipped = page.locator('.hl-node[data-run="skipped"]');
  await expect(skipped.filter({ hasText: "http_request" })).toHaveCount(1);
  await skipped.filter({ hasText: "http_request" }).locator(".hl-node__title").click();
  await expect(page.getByRole("complementary", { name: "Inspector" })).toContainText("not run: the coalesce after it already had a value");
});

test("an env secret is picked from the server's HOLON_SECRET_… names, its dotenv file read again for each", async ({ page, request, adminToken }) => {
  // Names only, to an admin: the e2e server's dotenv file (server.mjs) has one, beside a name it doesn't offer.
  const env = (await (await request.get(`${SERVER}/admin/secrets/env`, { headers: { Authorization: `Bearer ${adminToken}` } })).json()) as { names: string[]; dotenv: string };
  expect(env.names).toContain("HOLON_SECRET_GEMINI_API_KEY");
  expect(env.names).not.toContain("VAULT_TOKEN");
  // A line added while the server runs is offered the next time ⚙ opens, with no restart.
  const added = `HOLON_SECRET_E2E_${Date.now().toString(36).toUpperCase()}`;
  appendFileSync(env.dotenv, `${added}=e2e-added-value\n`);

  await openSample(page, "Gemini chat (a streamed reply, free tier)");
  // What the row sets is caught here: the gemini-chat test sets this secret on the same server.
  let posted: unknown = null;
  await page.route(`${SERVER}/deployments/gemini-chat/secrets/gemini_api_key`, (route) =>
    route.request().method() === "POST" ? ((posted = route.request().postDataJSON()), route.fulfill({ json: { version: 1 } })) : route.continue(),
  );
  await page.locator(".hl-canvas").getByRole("button", { name: "⚙", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "deploy settings" });
  await settings.locator("select", { has: page.locator('option[value="vault"]') }).selectOption("env");
  // The variable the secret's name suggests is chosen; the one just added is there too.
  const names = settings.locator("select", { has: page.locator("option", { hasText: "another name…" }) });
  await expect(names).toHaveValue("HOLON_SECRET_GEMINI_API_KEY");
  await expect(names.locator("option")).toContainText([added, "HOLON_SECRET_GEMINI_API_KEY"]);
  await names.selectOption(added);
  await settings.getByRole("button", { name: "set", exact: true }).click();
  await expect(settings).toContainText("version 1 set");
  expect(posted).toEqual({ from: `env:${added}` });
  // A name the server doesn't list is typed.
  await names.selectOption({ label: "another name…" });
  await expect(settings.getByPlaceholder("HOLON_SECRET_GEMINI_API_KEY")).toBeVisible();
});

test("calc's match answers the chosen arm or 400 when it has no value, and view mode shows the other arms as not run", async ({ page, request }) => {
  await openSample(page, "Calculator (a match)");
  const port = await deploy(page);
  // `a / b` has no value when b is 0; an arm not chosen doesn't matter.
  const add = await request.post(`http://127.0.0.1:${port}/calc`, { data: { op: "add", a: 1, b: 0 } });
  expect(add.status()).toBe(200);
  expect(await add.text()).toBe("1");
  // The chosen arm's absence is the match's: 400, not another arm's value.
  expect((await request.post(`http://127.0.0.1:${port}/calc`, { data: { op: "div", a: 1, b: 0 } })).status()).toBe(400);
  // The division ran; the other three arms didn't.
  await viewNewestRun(page);
  const skipped = page.locator('.hl-node[data-run="skipped"]');
  await expect(skipped).toHaveCount(3);
  for (const leaf of ["add", "subtract", "multiply"]) {
    await expect(skipped.filter({ has: page.locator(".hl-node__title", { hasText: new RegExp(`^${leaf}\\b`) }) })).toHaveCount(1);
  }
  await skipped.filter({ hasText: "subtract" }).locator(".hl-node__title").click();
  await expect(page.getByRole("complementary", { name: "Inspector" })).toContainText("not run: the guard after it didn't take this branch");

  // The text shows the run too (docs/L4s/text-graph-link-L4.md): the node picked on the canvas
  // marked, the values of the arms not chosen dimmed (their patterns ran), the values sent
  // beside `a` and `b`, the step marked, and the cursor picks the node under it.
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  const text = page.getByRole("region", { name: "Text" });
  await expect(text.locator(".hl-text__mark")).toHaveText("a - b");
  await expect(text.locator(".hl-text__dim")).toHaveText(["a + b", "a - b", "a * b"]);
  const values = text.locator(".hl-text__value");
  await expect(values.first()).toHaveText("1");
  await expect(values.nth(1)).toHaveText("0");
  await page.getByTitle("step forward one node").click();
  await expect(text.locator(".hl-text__step")).toHaveCount(1);
  await text.locator(".cm-line", { hasText: '"div" -> a / b' }).click();
  await page.keyboard.press("End");
  await expect(text.locator(".hl-text__mark")).toHaveText("a / b");
  await expect(page.getByRole("complementary", { name: "Inspector" })).toContainText("divide");
  await page.screenshot({ path: "test-results/text-run.png" });
});

test("inbox-report reads a file from a bound folder and writes its report to another, and its runs verify", async ({ page, request, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readFileSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const name = `sales-${Date.now()}.csv`;
  writeFileSync(join(FOLDERS, "inbox", name), "region,amount\nnorth,10\nsouth,32\n");
  await openSample(page, "Inbox report (files in folders on the host)");
  const port = await deploy(page);
  const made = await request.post(`http://127.0.0.1:${port}/report`, { data: { name } });
  expect(made.status()).toBe(200);
  expect(await made.json()).toEqual({ rows: 2, report: `${name}.md` });
  await viewNewestRun(page);
  // Written whole when the run finished, in the real folder; served and listed by the flow.
  const written = readFileSync(join(FOLDERS, "reports", `${name}.md`), "utf8");
  expect(written).toContain("| amount | 42 |");
  expect(await (await request.get(`http://127.0.0.1:${port}/reports/${name}.md`)).text()).toBe(written);
  expect(Object.keys(await (await request.get(`http://127.0.0.1:${port}/reports`)).json())).toContain(`${name}.md`);
  // No such file: no value. A name that leaves its folder: the run fails, writing nothing.
  expect((await request.post(`http://127.0.0.1:${port}/report`, { data: { name: "none.csv" } })).status()).toBe(404);
  expect((await request.post(`http://127.0.0.1:${port}/report`, { data: { name: "../reports/x.csv" } })).status()).toBe(500);

  // Its runs check offline: the file it read is in the log, what it wrote by its hash.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "inbox-report.json");
  execFileSync(holon, ["export", "inbox-report", "--server", SERVER, "-o", out], { encoding: "utf8", env: { ...process.env, HOLON_TOKEN: adminToken } });
  // A run that wrote the report keeps it by its hash (one that served it read it, and keeps that).
  const lines: { route?: string; performed: unknown; outputs: unknown }[] = JSON.parse(readFileSync(out, "utf8")).lines.map((l: string) => JSON.parse(l));
  const wrote = JSON.stringify(lines.filter((e) => e.route === "POST /report").map((e) => [e.performed, e.outputs]));
  expect(wrote).toContain("FileWrite");
  expect(wrote).not.toContain("| amount |");
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified).toContain("re-ran 4 runs: each produced what its log entry says");
  expect(verified).toContain("it failed: a file read failed:");
  expect(verified.trim().endsWith("ok")).toBe(true);
});

test("log-archive reads the last lines of a log too big to read whole and archives it by handle, and its runs verify", async ({ page, request, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readFileSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const name = `app-${Date.now()}.log`;
  const log = Array.from({ length: 60_000 }, (_, i) => `line ${i} of a log too big to read whole\n`).join("");
  writeFileSync(join(FOLDERS, "logs", name), log);
  await openSample(page, "Log archive (big files, by handle and by range)");
  const port = await deploy(page);
  const tail = await request.get(`http://127.0.0.1:${port}/logs/${name}/tail`);
  expect(tail.status()).toBe(200);
  const last: string[] = await tail.json();
  expect([last.length, last[19]]).toEqual([20, "line 59999 of a log too big to read whole"]);
  await viewNewestRun(page);
  // Copied host to host by its handle: the bytes never entered the run.
  const archived = await request.post(`http://127.0.0.1:${port}/archive`, { data: { name } });
  expect(archived.status()).toBe(200);
  const answer: { archived: string; size: number } = await archived.json();
  expect(answer.size).toBe(log.length);
  expect(readFileSync(join(FOLDERS, "archive", answer.archived), "utf8")).toBe(log);
  expect(Object.keys(await (await request.get(`http://127.0.0.1:${port}/archive`)).json())).toContain(answer.archived);

  // Its runs check offline from the lines they read, proved part of the log by its hashes.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "log-archive.json");
  execFileSync(holon, ["export", "log-archive", "--server", SERVER, "-o", out], { encoding: "utf8", env: { ...process.env, HOLON_TOKEN: adminToken } });
  const exported = readFileSync(out, "utf8");
  expect(exported).toContain("line 59999 ");
  expect(exported).not.toContain("line 30000 ");
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified).toContain("re-ran 3 runs: each produced what its log entry says");
  expect(verified.trim().endsWith("ok")).toBe(true);
});

test("journal appends to a page in a capped folder, a write past its cap answers 500, and its runs verify", async ({ page, request, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readFileSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const day = `day-${Date.now()}`;
  await openSample(page, "Journal (a folder kept for a while, and capped)");
  const port = await deploy(page);
  for (const line of ["first line", "second line"]) {
    expect((await request.post(`http://127.0.0.1:${port}/journal/${day}`, { data: line, headers: { "content-type": "text/plain" } })).status()).toBe(200);
  }
  const shown = await request.get(`http://127.0.0.1:${port}/journal/${day}`);
  expect(await shown.text()).toBe("first line\nsecond line\n");
  await viewNewestRun(page);

  // The folder is capped at 64 KiB: a page that would take it past that isn't written.
  const full = `full-${Date.now()}`;
  writeFileSync(join(FOLDERS, "journal", `${full}.txt`), "x".repeat(60 * 1024));
  const past = await request.post(`http://127.0.0.1:${port}/journal/${full}`, { data: "y".repeat(8 * 1024), headers: { "content-type": "text/plain" } });
  expect(past.status()).toBe(500);
  expect(await past.text()).toContain("couldn't be written");
  expect(readFileSync(join(FOLDERS, "journal", `${full}.txt`), "utf8")).toBe("x".repeat(60 * 1024));

  // Its texts are kept a week: a sweep now removes none a run refers to, and the runs verify.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const env = { ...process.env, HOLON_TOKEN: adminToken };
  expect(execFileSync(holon, ["folder", "sweep", "--server", SERVER], { encoding: "utf8", env })).toContain("swept 0 file texts");
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "journal.json");
  execFileSync(holon, ["export", "journal", "--server", SERVER, "-o", out], { encoding: "utf8", env });
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified).toContain("each produced what its log entry says");
  expect(verified.trim().endsWith("ok")).toBe(true);
});

test("shared-ledger, deployed twice, loses no line when both append to one file at once", async ({ page, request }) => {
  const { mkdtempSync, readFileSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  await openSample(page, "Shared ledger (one file, several deployments)");
  const first = await deploy(page);
  // A second deployment of the same text, under another name and port, imported as a file.
  const text = readFileSync(join(process.cwd(), "..", "crates", "holon-author", "samples", "shared-ledger.holon"), "utf8");
  const copy = join(mkdtempSync(join(tmpdir(), "holon-ledger-")), "shared-ledger-2.holon");
  writeFileSync(copy, text.replace("flow shared-ledger", "flow shared-ledger-2").replace("port 8140", "port 8141"));
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await page.getByLabel("import a .holon file").setInputFiles(copy);
  await expect(page.locator(".hl-tab--active")).toContainText("shared-ledger-2");
  const second = await deploy(page);
  expect(second).not.toBe(first);

  const tag = `e2e-${Date.now()}`;
  const lines = Array.from({ length: 30 }, (_, i) => `${tag} ${i}`);
  const answers = await Promise.all(
    lines.map((line, i) => request.post(`http://127.0.0.1:${i % 2 ? second : first}/lines`, { data: line, headers: { "content-type": "text/plain" } })),
  );
  expect(answers.map((a) => a.status())).toEqual(lines.map(() => 200));
  const kept = (await (await request.get(`http://127.0.0.1:${first}/lines`)).text()).split("\n").filter((l) => l.startsWith(tag));
  expect(kept.sort()).toEqual([...lines].sort());
  expect(readFileSync(join(FOLDERS, "ledger", "lines.txt"), "utf8")).toContain(`${tag} 29`);
});

test("orders-api places, lists and refunds orders in the database an admin bound; a second refund changes no row and answers 409", async ({ page, request }) => {
  const { join } = await import("node:path");
  await openSample(page, "Orders API (a database on the host)");
  const port = await deploy(page);
  const base = `http://127.0.0.1:${port}`;
  const customer = `e2e-${Date.now()}`;
  expect((await request.post(`${base}/orders`, { data: { customer, total: "12.50" } })).status()).toBe(201);
  const listed = await (await request.get(`${base}/orders/${customer}`)).json();
  expect(listed).toEqual([{ id: listed[0].id, total: 12.5, refunded: 0 }]);
  const id = listed[0].id;
  expect((await request.post(`${base}/order/${id}/refund`)).status()).toBe(200);
  const again = await request.post(`${base}/order/${id}/refund`);
  expect(again.status()).toBe(409);
  expect(await again.text()).toContain("changed 0 rows, not 1 row");
  expect((await (await request.get(`${base}/order/${id}`)).json()).refunded).toBe(1);

  // The admin page's resources list the database, its tables and who uses it, and bind another,
  // made by its schema.
  await page.getByRole("button", { name: "admin", exact: true }).click();
  const sections = page.getByRole("tablist", { name: "admin sections" });
  await sections.getByRole("tab", { name: "resources" }).click();
  await expect(page.getByText(join(FOLDERS, "shop.db"), { exact: true })).toBeVisible();
  await expect(page.getByLabel("shop's tables")).toContainText("tables: orders");
  await expect(page.getByLabel("shop's tables")).not.toContainText("holon_commits");
  const scratch = join(FOLDERS, `scratch-${Date.now()}.db`);
  await page.getByLabel("database name").fill("scratch");
  await page.getByLabel("database path").fill(scratch);
  await page.getByLabel("database schema").fill("create table notes (id integer primary key, text text)");
  await page.getByRole("button", { name: "bind database" }).click();
  await expect(page.getByText(scratch, { exact: true })).toBeVisible();
  await expect(page.getByLabel("scratch's tables")).toHaveText(/tables: notes/);
  // A URL typed in where its secret's source goes is refused here, never sent.
  await page.getByLabel("database path").fill("postgres://u:p@db.example/shop");
  await expect(page.getByRole("button", { name: "bind database" })).toBeDisabled();
  await page.getByLabel("database path").fill("");

  // Permissions: the policy, read only, and who may touch the shop: orders-api, by table, with
  // a link to its card, where its grant changes.
  await sections.getByRole("tab", { name: "permissions" }).click();
  await expect(page.getByRole("button", { name: "read the policy again" })).toBeVisible();
  const shop = page.getByLabel("database shop", { exact: true });
  await expect(shop).toContainText("orders-api");
  await expect(shop).toContainText("writes orders");
  await page.getByLabel("show what").selectOption("folders");
  await expect(shop).toHaveCount(0);
  await page.getByLabel("show what").selectOption("databases");
  await shop.getByRole("button", { name: "card →" }).first().click();
  await expect(page.getByRole("tablist", { name: "open tabs" }).getByRole("tab", { name: /^deployed/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByLabel("deployment orders-api", { exact: true })).toContainText("permissions ▾");
});

test("refund-once, beside orders-api on one database, calls the payment API once for two refunds of one order at once", async ({ page, request }) => {
  await openSample(page, "Orders API (a database on the host)");
  const orders = `http://127.0.0.1:${await deploy(page)}`;
  await openSample(page, "Refund once (two requests, one payment)");
  const refunds = `http://127.0.0.1:${await deploy(page)}`;
  const customer = `e2e-refund-${Date.now()}`;
  expect((await request.post(`${orders}/orders`, { data: { customer, total: 5 } })).status()).toBe(201);
  const [order] = await (await request.get(`${orders}/orders/${customer}`)).json();
  // The two at once: one holds the order's row from its call out until it commits; the other
  // waits, finds the order refunded, and sends nothing.
  const both = await Promise.all([request.post(`${refunds}/refund/${order.id}`, { timeout: 60_000 }), request.post(`${refunds}/refund/${order.id}`, { timeout: 60_000 })]);
  expect(both.map((r) => r.status()).sort()).toEqual([200, 409]);
  const made = await (await request.get(`${refunds}/refunds/${order.id}`)).json();
  expect(made).toHaveLength(1);
  expect(made[0].amount).toBe(5);
  expect((await (await request.get(`${orders}/order/${order.id}`)).json()).refunded).toBe(1);
});

test("ticket-numbers, on a database held for whole runs, hands out twenty numbers at once one after the other, none running again", async ({ page, request, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  await openSample(page, "Ticket numbers (a database held for whole runs)");
  const base = `http://127.0.0.1:${await deploy(page)}`;
  const tag = `e2e-${Date.now()}`;
  const taken = await Promise.all(Array.from({ length: 20 }, (_, i) => request.post(`${base}/ticket`, { data: { name: `${tag}-${i}` }, timeout: 60_000 })));
  expect(taken.map((t) => t.status())).toEqual(taken.map(() => 201));
  const numbers = (await Promise.all(taken.map((t) => t.json()))).map((t) => t.ticket);
  expect(new Set(numbers).size).toBe(20);
  const issued: { n: number; holder: string }[] = await (await request.get(`${base}/tickets`)).json();
  expect(issued.filter((t) => t.holder.startsWith(tag)).map((t) => t.n).sort((a, b) => a - b)).toEqual([...numbers].sort((a, b) => a - b));

  // Each run committed on its first try: the database was its alone from before it started.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const env = { ...process.env, HOLON_TOKEN: adminToken };
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "ticket-numbers.json");
  execFileSync(holon, ["export", "ticket-numbers", "--server", SERVER, "-o", out], { encoding: "utf8", env });
  const runs: { route?: string; retries?: number }[] = JSON.parse(readFileSync(out, "utf8")).lines.map((l: string) => JSON.parse(l));
  const posts = runs.filter((r) => r.route === "POST /ticket");
  expect(posts.length).toBeGreaterThanOrEqual(20);
  expect(posts.filter((r) => (r.retries ?? 0) > 0)).toEqual([]);

  // The admin page says the database is held for whole runs.
  await page.getByRole("button", { name: "admin", exact: true }).click();
  await page.getByRole("tablist", { name: "admin sections" }).getByRole("tab", { name: "resources" }).click();
  await expect(page.getByText(join(FOLDERS, "tickets.db"), { exact: true }).locator("..")).toContainText("held for whole runs");
});

test("approved-payout waits for its approval holding nothing, and pays from the balance as it is then: a deposit made meanwhile is kept", async ({ page, request }) => {
  await openSample(page, "Approved payout (a waiting run checks what it read)");
  const base = `http://127.0.0.1:${await deploy(page)}`;
  const before: number = (await (await request.get(`${base}/accounts/1`)).json()).balance;
  const asked = await request.post(`${base}/payout/1`, { data: { amount: 30 } });
  expect(asked.status()).toBe(202);
  const { approve } = await asked.json();
  expect(approve).toMatch(/^\/approve\/rt_/);
  // While it waits, a deposit goes ahead: the waiting run holds nothing on the account.
  const deposited = await request.post(`${base}/deposit/1`, { data: { amount: 50 } });
  expect((await deposited.json()).balance).toBe(before + 50);
  expect((await request.post(`${base}${approve}`, { data: { by: "e2e" }, timeout: 60_000 })).status()).toBe(200);
  expect((await (await request.get(`${base}/accounts/1`)).json()).balance).toBe(before + 50 - 30);
});

test("atlas-notes keeps notes as documents in a MongoDB: found by owner and tag, marked done once, and a value never an operator", async ({ page, request }) => {
  test.skip(!process.env.HOLON_TEST_MONGODB, "set HOLON_TEST_MONGODB (a replica set's URL) to run it against MongoDB");
  await openSample(page, ATLAS_NOTES);
  const base = `http://127.0.0.1:${await deploy(page)}`;
  const owner = `e2e-${Date.now()}`;
  expect((await request.post(`${base}/notes/${owner}`, { data: { id: `${owner}-1`, text: "buy milk", tags: ["home"] } })).status()).toBe(201);
  expect((await request.post(`${base}/notes/${owner}`, { data: { id: `${owner}-2`, text: "call bo" } })).status()).toBe(201);
  const found = await (await request.get(`${base}/notes/${owner}`)).json();
  expect(found.map((n: { text: string }) => n.text)).toEqual(["buy milk", "call bo"]);
  expect(await (await request.get(`${base}/notes/${owner}/tagged/home`)).json()).toHaveLength(1);
  expect((await request.post(`${base}/notes/${owner}/${owner}-1/done`)).status()).toBe(200);
  expect((await request.post(`${base}/notes/${owner}/${owner}-1/done`)).status()).toBe(404);
  // A value is data: a record with an operator in it fails the run, and nothing lands.
  expect((await request.put(`${base}/notes/${owner}/${owner}-2`, { data: { text: { $set: { owner: "mallory" } } } })).status()).toBe(500);
  expect((await (await request.get(`${base}/notes/${owner}`)).json())[0].text).toBe("call bo");
});

test("drop-box runs once for a file dropped in its folder, sends it by handle, writes its receipt, and its runs verify", async ({ page, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { existsSync, mkdtempSync, readFileSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  await openSample(page, "Drop box (a run per new file, sent on by handle)");
  await deploy(page);
  // A file dropped in: once it stops changing (a look or two, a second apart), one run sends it
  // and writes its receipt. Whether httpbin.org answered or not, the receipt says what was sent.
  const name = `dropped-${Date.now()}.csv`;
  const text = "region,amount\nnorth,10\nsouth,32\n";
  writeFileSync(join(FOLDERS, "drop", name), text);
  const receipt = join(FOLDERS, "receipts", `${name}.json`);
  await expect.poll(() => existsSync(receipt), { timeout: 15_000 }).toBe(true);
  const said = JSON.parse(readFileSync(receipt, "utf8"));
  expect(said.file).toBe(name);
  expect(said.size).toBe(text.length);
  expect(said.hash).toMatch(/^[0-9a-f]{64}$/);
  // A run with no answer to compare has no verdict; its replay still ran the nodes the run ran.
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).click();
  await expect(page.locator(".hl-view__bottom")).toContainText(`new file ${name}`);
  await expect(page.locator(".hl-view__bottom")).not.toContainText("replay differs from the run");

  // Its runs check offline: the file's handle is what arrived, the server's answer what the log says.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const env = { ...process.env, HOLON_TOKEN: adminToken };
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "drop-box.json");
  execFileSync(holon, ["export", "drop-box", "--server", SERVER, "-o", out], { encoding: "utf8", env });
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified).toContain("each produced what its log entry says");
  expect(verified.trim().endsWith("ok")).toBe(true);
});

test("file-audit runs on a csv dropped in the inbox and on a file removed from it, not on other names, and its runs verify", async ({ page, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync, existsSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  await openSample(page, "File audit (runs on several folders, removals and some names)");
  await deploy(page);
  const stamp = Date.now();
  const entries = () => (existsSync(join(FOLDERS, "audit", "inbox")) ? readdirSync(join(FOLDERS, "audit", "inbox")) : []);
  writeFileSync(join(FOLDERS, "inbox", `audit-${stamp}.csv`), "a,1\n");
  writeFileSync(join(FOLDERS, "inbox", `audit-${stamp}.txt`), "not a csv");
  await expect.poll(() => entries().filter((e) => e.endsWith(`-new-audit-${stamp}.csv.json`)).length, { timeout: 15_000 }).toBe(1);
  const entry = entries().find((e) => e.endsWith(`-new-audit-${stamp}.csv.json`))!;
  expect(JSON.parse(readFileSync(join(FOLDERS, "audit", "inbox", entry), "utf8"))).toMatchObject({ event: "new", folder: "inbox", name: `audit-${stamp}.csv`, size: 4 });

  // A file removed: one more entry, as it was last seen.
  rmSync(join(FOLDERS, "inbox", `audit-${stamp}.txt`));
  await expect.poll(() => entries().filter((e) => e.endsWith(`-deleted-audit-${stamp}.txt.json`)).length, { timeout: 15_000 }).toBe(1);
  expect(entries().some((e) => e.endsWith(`-new-audit-${stamp}.txt.json`)), "a .txt dropped in starts nothing").toBe(false);
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).click();
  await expect(page.locator(".hl-view__bottom")).toContainText(`deleted file audit-${stamp}.txt`);
  await expect(page.locator(".hl-view__bottom")).not.toContainText("replay differs from the run");

  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const env = { ...process.env, HOLON_TOKEN: adminToken };
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "file-audit.json");
  execFileSync(holon, ["export", "file-audit", "--server", SERVER, "-o", out], { encoding: "utf8", env });
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified.trim().endsWith("ok")).toBe(true);
});

test("the admin page's browse… lists the server's folders, and the one chosen fills the path", async ({ page }) => {
  const { realpathSync } = await import("node:fs");
  const { join } = await import("node:path");
  await page.goto("/");
  await page.getByRole("button", { name: "admin", exact: true }).click();
  await page.getByRole("tablist", { name: "admin sections" }).getByRole("tab", { name: "resources" }).click();
  // The first listing (the home folder) answers only once a path is typed, as on a slow machine:
  // it mustn't replace what was typed.
  let release = () => {};
  const held = new Promise<void>((r) => (release = r));
  let first = true;
  await page.route(/\/admin\/folders\/browse/, async (route) => {
    if (first) {
      first = false;
      await held;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "browse…" }).click();
  // The e2e server shows no OS dialog (HOLON_NO_FOLDER_DIALOG), so the editor lists the folders.
  const picker = page.getByRole("dialog", { name: "choose a folder" });
  await picker.getByLabel("folder to show").fill(FOLDERS);
  release();
  await expect(picker.getByRole("button", { name: "home", exact: true })).toBeVisible();
  await expect(picker.getByLabel("folder to show")).toHaveValue(FOLDERS);
  await picker.getByRole("button", { name: "go", exact: true }).click();
  await picker.getByLabel("folders").getByRole("button", { name: "▸ drop" }).click();
  await expect(picker.getByLabel("folder to show")).toHaveValue(realpathSync.native(join(FOLDERS, "drop")));
  await picker.getByRole("button", { name: "up", exact: true }).click();
  await expect(picker.getByLabel("folder to show")).toHaveValue(realpathSync.native(FOLDERS));
  await picker.getByLabel("folders").getByRole("button", { name: "▸ outbox" }).click();
  await picker.getByRole("button", { name: "Choose this folder" }).click();
  await expect(picker).toHaveCount(0);
  await expect(page.getByLabel("folder path")).toHaveValue(realpathSync.native(join(FOLDERS, "outbox")));
});

test("a deployment's runs export from view mode and from holon export, and holon verify runs each again", async ({ page, request, adminToken }) => {
  const { execFileSync, spawnSync } = await import("node:child_process");
  const { mkdtempSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  await openSample(page, "Ledger (every run checkable)");
  const port = await deploy(page);
  for (const amount of [5, 0.1, 0.2]) {
    const res = await request.post(`http://127.0.0.1:${port}/accounts/ada`, { data: { amount } });
    expect(res.ok()).toBe(true);
  }
  expect((await request.post(`http://127.0.0.1:${port}/accounts/ada`, { data: {} })).status()).toBe(400);
  await viewNewestRun(page);

  // Exported from the run log: the head it chains up to, and the file.
  const chain = page.locator(".hl-view__chain");
  await expect(chain).toContainText("chain unbroken");
  const head = (await chain.locator("[title]").filter({ hasText: /^[0-9a-f]{12}$/ }).getAttribute("title"))!;
  expect(head).toMatch(/^[0-9a-f]{64}$/);
  const dir = mkdtempSync(join(tmpdir(), "holon-runs-"));
  const download = page.waitForEvent("download");
  await chain.getByRole("button", { name: "export runs" }).click();
  const file = join(dir, "ledger-runs.json");
  await (await download).saveAs(file);
  await expect(chain).toContainText("holon verify ledger-runs.json");

  // Checked on the command line, each run run again: the `holon` the e2e server was built as.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const verified = execFileSync(holon, ["verify", file, "--head", head], { encoding: "utf8" });
  expect(verified).toContain("re-ran 4 runs: each produced what its log entry says");
  expect(verified).toContain("so no run up to it was changed or removed");
  expect(verified.trim().endsWith("ok")).toBe(true);

  // The same file from the command line, signed in with a token.
  const out = join(dir, "from-cli.json");
  const exported = execFileSync(holon, ["export", "ledger", "--server", SERVER, "-o", out], { encoding: "utf8", env: { ...process.env, HOLON_TOKEN: adminToken } });
  expect(exported).toContain(`head ${head.slice(0, 12)}`);
  expect(execFileSync(holon, ["verify", out], { encoding: "utf8" })).toContain("re-ran 4 runs");

  // And as the user running the servers, with no token: from their run files (local.rs). Both
  // test servers run, so it asks which; a port alone picks one.
  const local = { ...process.env, HOLON_RUN_DIR: RUN_DIR, HOLON_TOKEN: "", HOLON_SERVER: "" };
  const which = spawnSync(holon, ["export", "ledger"], { encoding: "utf8", env: local });
  expect(which.status).toBe(1);
  expect(which.stderr).toContain(`pick one with --server ${PORTS.server} or --server ${PORTS.fresh}`);
  // Into a folder of its own (made for it), by a name (.json added), the full path said.
  const mine = join(dir, "audits", "as-me.json");
  const asMe = execFileSync(holon, ["export", "ledger", "--server", String(PORTS.server), "--to", join(dir, "audits"), "--as", "as-me"], { encoding: "utf8", env: local });
  expect(asMe).toContain(`wrote ${mine}: 4 runs of "ledger", head ${head.slice(0, 12)}`);
  expect(asMe).toContain(`holon verify ${mine}`);
  expect(execFileSync(holon, ["verify", mine], { encoding: "utf8" })).toContain("re-ran 4 runs");
});
