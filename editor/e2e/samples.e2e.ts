// Every sample, through the real editor and a real server (playwright.config.ts): opened from the
// flows page, deployed through the consent screen, and, for the ones that answer requests here,
// sent one and replayed exactly in view mode. What CLAUDE.md asks of a sample by hand ("run live
// once"), kept from rotting by CI. A sample deploys on its own port plus 1000 (set under ⚙), so a
// developer's deployments on the samples' ports are left alone.

import type { Page } from "@playwright/test";
import { expect, FOLDERS, RUN_DIR, SERVER, test } from "./fixtures";

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
  // Wait for the server's answer to the deploy, which comes once the flow listens on its port
  // (http.rs syncs the listeners first). The tab's "deployed" can't say so: it shows as soon as a
  // deployment of that name exists, so on a redeploy that moves the port it is there at once.
  const deployed = page.waitForResponse((r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy");
  await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy|Update)$/ }).click();
  expect((await deployed).status()).toBe(200);
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

test("every sample opens from the flows page and deploys", async ({ page, request, adminToken }) => {
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

test("a sample deployed as it is keeps its text and the names it binds", async ({ page, request, adminToken }) => {
  // On its own port, so the text (which says `port 8115`) still stands for the canvas: a deploy
  // then carries `source` and `bindings` (useDeployFlow.ts), and a run explains as the text reads
  // (`name = req.params.name`, not `field_2 = …`). Changing the port under ⚙ is a canvas change
  // the text doesn't have, so the loop above, on +1000, deploys the canvas alone.
  await openSample(page, "Cache-aside (a branch not taken)");
  await deploy(page, 0);
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
  expect(which.stderr).toContain("pick one with --server 7520 or --server 7521");
  // Into a folder of its own (made for it), by a name (.json added), the full path said.
  const mine = join(dir, "audits", "as-me.json");
  const asMe = execFileSync(holon, ["export", "ledger", "--server", "7520", "--to", join(dir, "audits"), "--as", "as-me"], { encoding: "utf8", env: local });
  expect(asMe).toContain(`wrote ${mine}: 4 runs of "ledger", head ${head.slice(0, 12)}`);
  expect(asMe).toContain(`holon verify ${mine}`);
  expect(execFileSync(holon, ["verify", mine], { encoding: "utf8" })).toContain("re-ran 4 runs");
});
