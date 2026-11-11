// UI-layer tests for samples: the real editor, real browser, real server (playwright.config.ts).
// A sample opened from the flows page, deployed through the consent screen, and exercised in view
// mode, for what only the editor shows. Every sample is deployed and sent a request without the
// editor by sweep.e2e.ts.
// A sample deploys on its own port plus 1000 (set in the deployment settings panel), so a
// developer's deployments on the samples' ports are left alone, plus 100 for each slot past the
// first (ports.mjs), so runs side by side don't meet.
//
// Pure-HTTP tests (no view mode or editor UI after deploy) live in system.e2e.ts: they deploy via
// `holon deploy` (no browser), so pointer-event layout changes can't break them.

import { appendFileSync, readFileSync } from "node:fs";
import { expect, FOLDERS, PORTS, RUN_DIR, SAMPLE_SHIFT, SAMPLES_RS, SERVER, test } from "./fixtures";
import { flowRow, gotoPage, navLink, notices, openSample, resourcesTab, still, openDeployPanel, deploy, settingsSayPort, dropFromPalette, viewNewestRun } from "./helpers";

const LEDGER_WATCH = "Ledger watch (a Postgres channel as a feed)";

/** The samples' titles, as holon-author lists them (samples.rs). */
const SAMPLE_TITLES = [...readFileSync(SAMPLES_RS, "utf8").matchAll(/^\s*title: "(.*)",$/gm)].map((m) => m[1]);

test("the library lists every sample", async ({ page }) => {
  await page.goto("/");
  await gotoPage(page, "library");
  const samples = page.getByRole("region", { name: "samples" });
  const listed = await samples.getByRole("article").getByRole("heading").allInnerTexts();
  // Every sample samples.rs has, and nothing else.
  expect(SAMPLE_TITLES.length).toBeGreaterThan(5);
  expect([...listed].sort()).toEqual([...SAMPLE_TITLES].sort());
});

test("shared-key reads the server's secret, which may go only where an admin said: sent elsewhere, the deploy is refused", async ({ page }) => {
  await openSample(page, "One key for every flow, capped");
  await openDeployPanel(page);
  const settings = page.getByRole("complementary", { name: "Deployment settings" });
  await expect(settings).toContainText("the server's · may go only to https://httpbin.dev");
  // The text changed to send it elsewhere: the deploy is refused, saying why.
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).dispatchEvent("click");
  const text = readFileSync(new URL("../../crates/holon-author/samples/shared-key.holon.ts", import.meta.url), "utf8");
  await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(text.replace('"host:httpbin.dev"', '"host:postman-echo.com"').replace('"/bearer"', '"/get"'));
  await expect(page.locator(".hl-node__secret").filter({ hasText: "postman-echo.com" })).toHaveCount(1);
  await page.locator(".hl-canvas").getByRole("button", { name: /^(deploy|update)/ }).dispatchEvent("click");
  await expect(page.getByText(`the server's secret "ops_api_key" may go only to https://httpbin.dev, and this flow sends it to https://postman-echo.com`)).toBeVisible();
  // The admin's list says where it may go.
  await gotoPage(page, "resources");
  await resourcesTab(page, "Secrets");
  await expect(page.getByLabel("server secret ops_api_key")).toContainText("may go only to https://httpbin.dev");
});

test("a sample deployed as it is keeps its text and the names it binds", async ({ page, request, adminToken }) => {
  // The port moved in the text (the text's wins while it is open), so the text still stands for
  // the canvas: a deploy then carries `source` and `bindings` (useDeployFlow.ts), and a run explains
  // as the text reads (`name = req.params.name`, not `field_2 = …`). Changing the port under ⚙
  // alone is a canvas change the text doesn't have, so the loop above deploys the canvas alone.
  await openSample(page, "Cache-aside (a branch not taken)");
  const port = 8115 + SAMPLE_SHIFT;
  const text = readFileSync(new URL("../../crates/holon-author/samples/cache-aside.holon.ts", import.meta.url), "utf8");
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).dispatchEvent("click");
  await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(text.replace("port: 8115", `port: ${port}`));
  await settingsSayPort(page, String(port));
  expect(await deploy(page, 0)).toBe(port);
  const deployed = await request.get(`${SERVER}/deployments/cache-aside/document`, { headers: { Authorization: `Bearer ${adminToken}` } });
  expect(deployed.status()).toBe(200);
  const document = (await deployed.json()) as { source?: string; bindings?: Record<string, string> };
  expect(document.source).toContain('flow("cache-aside"');
  expect(Object.values(document.bindings ?? {})).toEqual(expect.arrayContaining(["name", "id"]));
});

test("a flow nothing would start can't deploy; given an action, it deploys and runs by hand", async ({ page }) => {
  await page.goto("/");
  await gotoPage(page, "new");
  const canvas = page.locator(".hl-canvas");
  // A leaf under no handler: nothing would start a run of it.
  await dropFromPalette(page, "uppercase");
  await expect(canvas.getByText("nothing would start a run", { exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(canvas.getByRole("button", { name: /^deploy/ })).toBeDisabled();
  // Written as an action, which a person runs by hand, it starts.
  await canvas.getByRole("button", { name: "text off" }).click();
  await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText('import { action, flow } from "holon"\n\nflow("by-hand")\n\nexport const run = action(() => 1 + 2)\n');
  await expect(canvas.getByText("nothing would start a run", { exact: true })).toHaveCount(0, { timeout: 15_000 });
  // Deployed under a name of its own (asked, as for any new flow), its action answers. With the
  // text open the canvas's toolbar runs under the text pane at 1280px: pressed as a button, not at
  // a point another element covers.
  await expect(canvas.getByRole("button", { name: /^deploy/ })).toBeEnabled();
  await canvas.getByRole("button", { name: /^deploy/ }).dispatchEvent("click");
  await page.locator('[aria-modal="true"]').locator("input").fill(`by-hand-${Date.now()}`);
  await page.keyboard.press("Enter");
  const deployed = page.waitForResponse((r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy");
  await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy)$/ }).click();
  expect((await deployed).status()).toBe(200);
  await page.getByRole("button", { name: "run", exact: true }).click();
  await expect(page.getByText(/^run: 3 \(run \d+\)/)).toBeVisible();
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
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).dispatchEvent("click");
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
  // Selected by its click event, as every other node in the suite is: a pointer click waits for
  // the node to hold still where the mouse goes, and on a slow worker it hung, or landed beside it.
  await page.locator(".react-flow__node", { hasText: "concat" }).dispatchEvent("click");
  await expect(inspector).toContainText("concat");
  await expect(inspector).toContainText("inputs");
  // The node's line in the flow as text (docs/LANGUAGE.md): the whole route, since concat is
  // part of the template literal it answers.
  await expect(inspector).toContainText("as text, line");
  await expect(inspector.locator(".hl-inspector__source")).toHaveText('route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)');
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
  await page.locator(".react-flow__node", { hasText: "publish" }).dispatchEvent("click");
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
  await flow.locator(".react-flow__node", { hasText: "publish" }).dispatchEvent("click");
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "expand" }).dispatchEvent("click");
  const overlay = page.locator(".hl-overlay");
  await expect(overlay.locator(".hl-overlay__crumbs")).toContainText("publish");
  await expect(overlay.locator(".react-flow__node", { hasText: "collection_put" })).toBeVisible();
  // Its boundary says what it is wired to in the flow around it.
  await expect(overlay.locator(".react-flow__node", { hasText: "inputs" })).toContainText("← slugify");
  await page.keyboard.press("Escape");
  await expect(overlay).toHaveCount(0);
  // Ungroup replaces it with its nodes, here.
  await flow.locator(".react-flow__node", { hasText: "publish" }).dispatchEvent("click");
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "ungroup" }).dispatchEvent("click");
  await expect(flow.locator(".react-flow__node", { hasText: "publish" })).toHaveCount(0);
  await expect(flow.locator(".react-flow__node", { hasText: "collection_put" })).toHaveCount(1);
});

test("an edited holon applies as a new version of itself, and the flow runs it", async ({ page, request }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  await flow.locator(".react-flow__node", { hasText: "publish" }).dispatchEvent("click");
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "expand" }).dispatchEvent("click");
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
  await expect(page.getByRole("region", { name: "notices" })).toContainText("is a new version");
  const port = await deploy(page);
  const answer = await request.post(`http://127.0.0.1:${port}/slugs`, { data: { title: "Edited Inside", author: "Ada" } });
  expect(await answer.json()).toMatchObject({ title: "edited-inside", receipt: "not signed" });
});

test("a port added inside a holon takes its first wire's type, and the flow's node gains it", async ({ page }) => {
  await openSample(page, "A flow inside a flow (holon)");
  const flow = page.locator(".hl-canvas__flow > .react-flow");
  await flow.locator(".react-flow__node", { hasText: "publish" }).dispatchEvent("click");
  await page.locator(".react-flow__node-toolbar").getByRole("button", { name: "expand" }).dispatchEvent("click");
  const overlay = page.locator(".hl-overlay");
  await overlay.getByRole("button", { name: "add an output" }).dispatchEvent("click");
  await page.locator('[aria-modal="true"] input').fill("stamp");
  await page.keyboard.press("Enter");
  const outputs = overlay.locator(".react-flow__node", { hasText: "outputs" });
  const stamp = outputs.locator(".hl-port__label", { hasText: "stamp" });
  await expect(stamp).toHaveAttribute("title", /^any/);
  // Wired from the clock: it's a number now.
  const clockOut = overlay.locator(".react-flow__node", { hasText: "clock" }).locator(".react-flow__handle.source");
  const stampIn = outputs.locator('.react-flow__handle[data-handleid="stamp"]');
  // Beside the inspector the overlay is about 280 px wide at the test's window: fitted, the whole
  // level is in it, the new port too, so a person can reach it without panning. (It once wasn't:
  // the zoom's floor left the outputs node past the overlay's edge.) A wire is made by clicking the
  // source handle and then the target's, which moves no pointer and pans nothing.
  await overlay.getByRole("button", { name: "Fit View" }).click();
  await still(clockOut);
  await still(stampIn);
  const inside = (await overlay.locator(".react-flow").boundingBox())!;
  for (const handle of [clockOut, stampIn]) {
    const at = (await handle.boundingBox())!;
    expect(at.x >= inside.x && at.x + at.width <= inside.x + inside.width && at.y >= inside.y && at.y + at.height <= inside.y + inside.height, "the handle is inside the overlay").toBe(true);
  }
  await clockOut.click();
  await stampIn.click();
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
    await gotoPage(page, "new");
    await expect(page.locator(".hl-tab--active")).toContainText("untitled");
    await dropFromPalette(page, entry);
    await expect(flow.locator(".react-flow__node")).toHaveCount(1);
    await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
    await ask.locator("input").fill(name);
    await page.keyboard.press("Enter");
    await expect(page.getByRole("region", { name: "notices" })).toContainText("saved");
    await expect(page.locator(".hl-tab--active")).toContainText(name);
  };
  await saveWith(shout, "uppercase");
  await saveWith(user, `flow:${shout}`);
  // A draft named on save as a flow saved already asks before replacing it; cancelled, it stays untitled.
  await gotoPage(page, "new");
  await dropFromPalette(page, "concat");
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await ask.locator("input").fill(shout);
  await page.keyboard.press("Enter");
  await expect(ask).toContainText(`A flow named "${shout}" is saved already`);
  await ask.getByRole("button", { name: "Cancel" }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("untitled");
  await gotoPage(page, "flows");
  await expect(page.getByText(`placed in ${user}`, { exact: true })).toBeVisible();
  // A save of "shout" that takes its input away asks first, naming who places it. Its tab is
  // the editor's.
  await navLink(page, "Editor").click();
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
  await gotoPage(page, "flows");
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
  await gotoPage(page, "new");
  await dropFromPalette(page, "uppercase");
  await expect(flow.locator(".react-flow__node")).toHaveCount(1);
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await answer(shout);
  await expect(page.getByRole("region", { name: "notices" })).toContainText("saved");

  // Set the marketplace on the Library's tab, and publish from Flows.
  await page.goto("/#library.market");
  await page.getByRole("button", { name: /^(set|change)$/ }).click();
  await answer(market);
  await expect(page.getByText(`publishing to and placing from ${market}`)).toBeVisible();
  await gotoPage(page, "flows");
  await flowRow(page, shout).getByRole("button", { name: "publish" }).click();
  await answer("shouts its text");
  await answer("e2e");
  await answer(key);
  await expect(page.getByText(`published "${shout}"`)).toBeVisible();
  const listings = (await (await request.get(`${market}/listings`)).json()) as Record<string, { id: string; name: string }>;
  const listed = Object.values(listings).find((l) => l.name === shout)!;

  // In another flow, from the palette's marketplace: seen what it may do, placed.
  await gotoPage(page, "new");
  await expect(page.locator(".hl-palette")).toContainText(shout);
  await dropFromPalette(page, `market:${listed.id}`);
  await expect(ask).toContainText("It only computes");
  await ask.getByRole("button", { name: "Place" }).click();
  await expect(flow.locator(".react-flow__node", { hasText: shout })).toHaveCount(1);

  // Yanked from the Library's marketplace: flagged, never edited, and the palette says so.
  await page.goto("/#library.market");
  const row = page.getByRole("region", { name: "marketplace" }).getByRole("listitem").filter({ has: page.getByText(shout, { exact: true }) }).filter({ hasText: "by e2e" }).last();
  await row.getByRole("button", { name: "yank" }).click();
  await ask.getByRole("button", { name: "Yank" }).click();
  await expect(row).toContainText("yanked");
  const after = (await (await request.get(`${market}/listings/${encodeURIComponent(listed.id)}`)).json()) as { yanked?: boolean; units: string[] };
  expect(after.yanked).toBe(true);
  expect(after.units.length).toBeGreaterThan(0);
});

test("a flow as text: exported as .holon.ts, checked by the command line, imported back", async ({ page }) => {
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, readFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const name = `texty-${Date.now().toString(36)}`;
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
  await expect(page.locator(".hl-tab--active")).toContainText(name);

  // Exported from the flows page: the canvas printed as TypeScript.
  await gotoPage(page, "flows");
  const row = flowRow(page, name);
  const download = page.waitForEvent("download");
  await row.getByRole("button", { name: ".holon.ts" }).click();
  const file = join(mkdtempSync(join(tmpdir(), "holon-text-")), `${name}.holon.ts`);
  await (await download).saveAs(file);
  const text = readFileSync(file, "utf8");
  expect(text).toContain(`flow("${name}")`);
  expect(text).toContain(".toUpperCase()");

  // Checked on the command line, with no browser: the `holon` the e2e server was built as.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const checked = execFileSync(holon, ["check", file], { encoding: "utf8" });
  expect(checked).toContain(`${name}: 3 nodes, 3 wires`);
  expect(checked).toContain("  action shout");

  // Imported back: an unsaved tab of its own, with the same node.
  await page.getByLabel("import a flow's text").setInputFiles(file);
  await expect(page.locator(".hl-tab--active")).toContainText(name);
  await expect(page.locator(".hl-canvas__flow > .react-flow .react-flow__node", { hasText: "uppercase" })).toHaveCount(1);
});


test("ticker's clock ticks beside its routes: its count grows on its own, tick now adds one, its action sets it by hand, from the editor and its page, and each run replays exactly", async ({ page, request }) => {
  await openSample(page, "Ticker (a clock beside routes)");
  // Agents may run its action, as its deploy settings ask: a grant of its own.
  const port = await deploy(page, SAMPLE_SHIFT, (settings) => settings.getByLabel("agents may run reset").check());
  const count = async () => Number(await (await request.get(`http://127.0.0.1:${port}/count`)).text());
  // Its every block runs on the scheduler, every 2 seconds, with no request to start it.
  await expect.poll(count, { timeout: 15_000 }).toBeGreaterThan(0);
  const before = await count();
  await page.getByRole("button", { name: "tick now" }).click();
  await expect(page.getByText(/^every 2s: ticked, 2 effects performed$/)).toBeVisible();
  expect(await count()).toBeGreaterThan(before);
  // The newest run is a tick, named by its clock, and view mode replays it over its block.
  await page.getByRole("button", { name: "tick now" }).click();
  await expect(page.getByText(/^every 2s: ticked/)).toBeVisible();
  await viewNewestRun(page);
  await expect(page.locator(".hl-view__bottom")).toContainText("every 2s");
  await page.locator(".hl-canvas").getByRole("button", { name: "edit", exact: true }).click();
  // Its action, run by hand from the server panel with what to send, sets the count: the clock
  // goes on from there.
  await page.getByLabel("what to send reset").fill('{"to": 1000}');
  await page.getByRole("button", { name: "run", exact: true }).click();
  await expect(page.getByText(/^reset: \{"set":1000,"was":\d+\} \(run \d+\)/)).toBeVisible();
  const reset = await count();
  expect(reset >= 1000 && reset < 1010, `${reset}`).toBe(true);
  // Its run is logged under its label: view run opens view mode on it, replayed over its block.
  await page.getByRole("button", { name: "view run" }).click();
  await expect(page.locator(".hl-view__bottom")).toContainText(/· action reset · session/);
  await expect(page.locator(".hl-view__bottom")).toContainText("replay reproduces the run");
  // Its page runs it too, without opening the flow; its grant lets agents run it.
  await page.goto("/#ticker");
  const actions = page.getByRole("region", { name: "actions" });
  await actions.getByLabel("what to send reset").fill('{"to": 5}');
  await actions.getByRole("button", { name: "run", exact: true }).click();
  await expect(actions.getByText(/^reset: \{"set":5,"was":\d+\} \(run \d+\)/)).toBeVisible();
  expect(await count()).toBeLessThan(1000);
  await expect(page.getByRole("region", { name: "it may" })).toContainText("let agents run its action reset");
});

test("shop runs billing's actions: an order calls billing and stores what it answered, a refund undoes it, a stopped billing answers 502, and the order's run replays exactly", async ({ page, request }) => {
  // billing first: the shop's calls need it deployed.
  await openSample(page, "Billing (actions another flow runs)");
  const billing = await deploy(page);
  await openSample(page, "Shop (a flow calling another's actions)", false);
  // The shop asks to run billing's actions, which its approval says in words.
  const shop = await deploy(page);
  const at = (port: number | null, path: string) => `http://127.0.0.1:${port}${path}`;
  const id = `o-${Date.now()}`;
  const placed = await request.post(at(shop, "/orders"), { data: { id, amount: 5 } });
  expect(placed.status()).toBe(201);
  expect(await placed.json()).toEqual({ id, total: 6 });
  expect(await (await request.get(at(shop, `/orders/${id}`))).json()).toEqual({ amount: 5, total: 6 });
  // The charge is billing's own: its page reads it back.
  expect(await (await request.get(at(billing, `/charges/${id}`))).json()).toEqual({ amount: 5, fee: 1 });
  // A refund runs billing's other action, which only writes.
  expect((await request.delete(at(shop, `/orders/${id}`))).status()).toBe(204);
  expect((await request.get(at(billing, `/charges/${id}`))).status()).toBe(404);
  // The shop's run replays exactly, the call answered from its record.
  await request.post(at(shop, "/orders"), { data: { id: `${id}-again`, amount: 7 } });
  await viewNewestRun(page);
  await expect(page.locator(".hl-view__bottom")).toContainText("replay reproduces the run");
  await page.locator(".hl-canvas").getByRole("button", { name: "edit", exact: true }).click();
  // Its page names what it may run.
  await page.goto("/#shop");
  await expect(page.getByRole("region", { name: "it may" })).toContainText("run the action charge of the flow billing");
  // A billing that isn't running can't be called: the call answers why, and the order isn't stored.
  await page.goto("/#billing");
  const head = page.getByLabel("flow billing");
  await head.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(head.locator(".hl-flow__status")).toContainText("stopped");
  const lost = `${id}-lost`;
  expect((await request.post(at(shop, "/orders"), { data: { id: lost, amount: 1 } })).status()).toBe(502);
  expect((await request.get(at(shop, `/orders/${lost}`))).status()).toBe(404);
});

test("drop-board's file handlers run beside its routes: a file dropped in, and one removed, show on its page, and a file's run replays exactly", async ({ page, browser }) => {
  const { rmSync, writeFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  await openSample(page, "Drop board (files on a live page)");
  const port = await deploy(page);
  // The server panel lists its file handlers beside its routes.
  await expect(page.getByText("board deleted", { exact: true })).toBeVisible();
  const board = await browser.newPage();
  const subscribed = board.waitForResponse((r) => new URL(r.url()).pathname === "/live");
  await board.goto(`http://127.0.0.1:${port}/`);
  await subscribed;
  // A file that settles in the folder runs the block for new files; removed, the one for deleted.
  const name = `dropped-${Date.now()}.txt`;
  writeFileSync(join(FOLDERS, "board", name), "hello");
  await expect(board.locator("li.new").first()).toHaveText(`new ${name} (5 bytes)`, { timeout: 15_000 });
  rmSync(join(FOLDERS, "board", name));
  await expect(board.locator("li.new").first()).toHaveText(`deleted ${name}`, { timeout: 15_000 });
  await board.close();
  // The newest run is the deleted file's, named by its line, and view mode replays it over its block.
  await viewNewestRun(page);
  await expect(page.locator(".hl-view__bottom")).toContainText(`deleted file ${name} (on file board deleted)`);
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

test("price-rollback: an earlier version's code restored from its page's history answers as it did, its prices kept, then its prices restored", async ({ page, request, adminToken }) => {
  await openSample(page, "Price rollback (an earlier version's code, or its prices, back)");
  const port = await deploy(page);
  const tea = `tea-${Date.now()}`;
  expect((await request.put(`http://127.0.0.1:${port}/prices/${tea}`, { data: { price: 4 } })).status()).toBe(200);
  const total = async () => ((await (await request.get(`http://127.0.0.1:${port}/quote/${tea}`)).json()) as { total: number }).total;
  expect(await total()).toBe(4.5);
  // Another version: its fee changed in the text, on the same port.
  const text = readFileSync(new URL("../../crates/holon-author/samples/price-rollback.holon.ts", import.meta.url), "utf8");
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(text.replace("port: 8160", `port: ${port}`).replace("const fee = 0.5", "const fee = 5"));
  // Compiled: the canvas is no longer what is deployed. (Its port already was: ⚙ saying so
  // doesn't show the new text has landed.)
  await expect(page.locator(".hl-canvas").getByTitle("the canvas has changed since it was deployed")).toBeVisible({ timeout: 15_000 });
  await settingsSayPort(page, String(port));
  expect(await deploy(page, 0)).toBe(port);
  expect(await total()).toBe(9);
  const auth = { Authorization: `Bearer ${adminToken}` };
  const listed = await request.get(`${SERVER}/deployments/price-rollback/versions`, { headers: auth });
  const versions = ((await listed.json()) as { versions: { number: number; current: boolean }[] }).versions;
  expect(versions[0].current).toBe(true);
  const [now, before] = [versions[0].number, versions[1].number];
  // The one before it, picked in its page's history, its code restored with the button: a dry
  // run first, confirmed in a dialog; then a new version naming it.
  await page.goto("/#price-rollback.history");
  const bottom = page.getByRole("region", { name: "history" });
  const pickBefore = () => bottom.locator(".hl-history__row").filter({ hasText: new RegExp(`deployed version ${before}(?!\\d)`) }).first().click();
  await pickBefore();
  const goBack = bottom.getByRole("region", { name: `go back to version ${before}` });
  await expect(goBack).toContainText(`Go back to version ${before}`);
  // Its details beside it, no run shown.
  await expect(bottom.locator(".hl-history__details")).toContainText(`deployed version ${before}`);
  await goBack.getByRole("radio", { name: "Code only" }).click();
  await expect(goBack).toContainText(`It deploys version ${before}'s code again, as a new version; the state stays as it is.`);
  await goBack.getByRole("button", { name: "Go back…" }).click();
  const dialog = page.getByRole("dialog", { name: "dry run" });
  await expect(dialog).toContainText("Nothing has changed yet: this is what it would do.");
  await expect(dialog).toContainText(`Version ${before}'s code is deployed again, as a new version`);
  // What it changes, a line each; its diff, a review in hunks: the text deployed now beside
  // version before's (both printed plainly: the version before was deployed from the canvas, which
  // keeps no names), and the grant.
  await expect(dialog.locator(".hl-dryrun__changes")).toContainText(`code: version ${before}'s is deployed again, as version ${now + 1}, in place of version ${now}`);
  const codeDiff = dialog.getByRole("region", { name: "code" });
  await expect(codeDiff.getByRole("button").first()).toContainText("+1 · −1");
  await expect(codeDiff.locator(".hl-review__row--del .hl-review__text")).toHaveText(["  const c = 5"]);
  await expect(codeDiff.locator(".hl-review__row--add .hl-review__text")).toHaveText(["  const c = 0.5"]);
  await expect(codeDiff).toContainText(`version ${before} was deployed from the canvas, which keeps none`);
  await expect(dialog.getByRole("region", { name: "permissions" })).toContainText('CollectionRead("prices")');
  await dialog.getByRole("button", { name: "Restore", exact: true }).click();
  await expect(bottom.locator(".hl-history__note")).toHaveText(`restored: version ${before}'s code is deployed again, as version ${now + 1}`);
  expect(await total()).toBe(4.5);
  // A wrong price, then the prices that version left put back with the state button: the dialog
  // says what it changes; it starts a new session, its code as it is.
  expect((await request.put(`http://127.0.0.1:${port}/prices/${tea}`, { data: { price: 40 } })).status()).toBe(200);
  expect(await total()).toBe(40.5);
  await pickBefore();
  await goBack.getByRole("radio", { name: "State only" }).click();
  await goBack.getByRole("button", { name: "Go back…" }).click();
  await expect(dialog).toContainText(`The state version ${before} left is put back: it changes the collection prices.`);
  // Its diff: the collection's items that differ, as the flow reads them, not its root.
  await expect(dialog.locator(".hl-dryrun__changes")).toContainText(`state: as version ${before} left it, 1 key changed (prices: 1 item)`);
  const stateDiff = dialog.getByRole("region", { name: "state" });
  await expect(stateDiff.locator(".hl-review__row--del .hl-review__text")).toHaveText([`prices["${tea}"]: 40`]);
  await expect(stateDiff.locator(".hl-review__row--add .hl-review__text")).toHaveText([`prices["${tea}"]: 4`]);
  expect(await total()).toBe(40.5);
  await dialog.getByRole("button", { name: "Restore", exact: true }).click();
  await expect(bottom.locator(".hl-history__note")).toContainText(`the state version ${before} left is live again, in session`);
  expect(await total()).toBe(4.5);
  // The history shows it, newest first, by whom; picked, its details say what it put back.
  const restored = bottom.locator(".hl-history__row").first();
  await expect(restored).toContainText(`put back the state version ${before} left`);
  await restored.click();
  await expect(bottom.locator(".hl-history__details")).toContainText(`the state version ${before} left`);
  // A wrong price again, and ↺ on the restore goes back to it: the prices it put back.
  expect((await request.put(`http://127.0.0.1:${port}/prices/${tea}`, { data: { price: 40 } })).status()).toBe(200);
  expect(await total()).toBe(40.5);
  await restored.locator(".hl-history__back").click();
  await expect(dialog).toContainText(`As it was after "put back the state version ${before} left`);
  await expect(dialog).toContainText(`The state version ${before} left is put back: it changes the collection prices.`);
  await dialog.getByRole("button", { name: "Restore", exact: true }).click();
  await expect(bottom.locator(".hl-history__note")).toContainText(`the state version ${before} left is live again, in session`);
  expect(await total()).toBe(4.5);
  // Each deploy and restore is on its timeline, by whom.
  const events = await request.get(`${SERVER}/deployments/price-rollback/events`, { headers: auth });
  const said = ((await events.json()) as { events: { kind: string; what: string; by?: string }[] }).events.slice(-5);
  expect(said.map((e) => e.kind)).toEqual(["deploy", "deploy", "deploy", "restore", "restore"]);
  expect(said[2].what).toBe(`restored version ${before}'s code, as version ${now + 1}`);
  expect(said.every((e) => typeof e.by === "string")).toBe(true);
});

test("price-rules: a new version is replayed on its recent runs before it goes live, in the deploy and in its review", async ({ page, request }) => {
  await openSample(page, "Price rules (a new version replayed on recent runs)");
  const port = await deploy(page);
  const price = (order: object) => request.post(`http://127.0.0.1:${port}/price`, { data: order });
  expect(await (await price({ total: 80, member: true })).json()).toEqual({ total: 80, price: 72 });
  expect(await (await price({ total: 80 })).json()).toEqual({ total: 80, price: 80 });
  expect((await price({})).status()).toBe(422);
  // A bigger discount, in the text: before it goes live, the deploy replays it on those runs.
  const text = readFileSync(new URL("../../crates/holon-author/samples/price-rules.holon.ts", import.meta.url), "utf8").replace("port: 8185", `port: ${port}`);
  const typed = async (discount: string) => {
    await page.getByRole("region", { name: "Text" }).locator(".cm-content").click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text.replace("total * 0.9", `total * ${discount}`));
    await expect(page.locator(".hl-canvas").getByTitle("the canvas has changed since it was deployed")).toBeVisible({ timeout: 15_000 });
    await settingsSayPort(page, String(port));
  };
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  await typed("0.85");
  await page.locator(".hl-canvas").getByRole("button", { name: /^(deploy|update)/ }).dispatchEvent("click");
  const dialog = page.getByRole("dialog");
  const replayed = dialog.getByRole("group", { name: "on recent runs" });
  await expect(replayed.getByLabel("replay summary")).toHaveText("3 replayed · 2 same · 1 differs");
  await expect(replayed.locator(".hl-replay__run--differs")).toContainText("its answer");
  await expect(replayed.locator(".hl-replay__run--differs")).toContainText("POST /price");
  // Nothing was sent or changed: the version deployed still answers 72.
  expect(await (await price({ total: 80, member: true })).json()).toEqual({ total: 80, price: 72 });
  await dialog.getByRole("button", { name: /^(Approve and deploy|Update)$/ }).click();
  await expect.poll(async () => (await (await price({ total: 80, member: true })).json()).price).toBe(68);
  // Another, saved: its review replays it on the runs too. A save waits for a person when it is
  // newer than the deploy, both kept to the second: a second later.
  const deployedAt = Date.now();
  await typed("0.8");
  await page.waitForTimeout(Math.max(0, 1_100 - (Date.now() - deployedAt)));
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).dispatchEvent("click");
  const name = page.locator('[aria-modal="true"] input');
  if (await name.isVisible({ timeout: 2_000 }).catch(() => false)) {
    await name.fill("price-rules");
    await page.keyboard.press("Enter");
  }
  await expect(notices(page)).toContainText("saved");
  await page.getByRole("navigation", { name: "main" }).getByRole("link", { name: /^Approvals/ }).click();
  await page.getByRole("region", { name: "deploys" }).getByRole("row").filter({ hasText: "price-rules" }).getByRole("link", { name: "review price-rules" }).click();
  const card = page.getByRole("group", { name: "on recent runs" });
  await expect(card.getByLabel("replay summary")).toHaveText(/^\d+ replayed · \d+ same · \d+ differs?/);
  await expect(card.locator(".hl-replay__run--differs").first()).toContainText("its answer");
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

test("room's pages talk over WebSockets: what one says reaches both, and the other hears who left", async ({ page, browser }) => {
  await openSample(page, "Room (a chat over WebSockets)");
  const port = await deploy(page);
  // A page in the room under `name`: its WebSocket on /room/<name>, open once it says so.
  const join = async (name: string) => {
    const tab = await browser.newPage();
    await tab.goto(`http://127.0.0.1:${port}/`);
    await tab.getByPlaceholder("Your name").fill(name);
    await tab.getByRole("button", { name: "Join" }).click();
    await expect(tab.locator("#said")).toContainText(`you joined as ${name}`);
    return tab;
  };
  const ann = await join("ann");
  const bob = await join("bob");
  await expect(ann.locator("#said")).toContainText("bob joined");
  // What one says runs its message block, and its pack reaches both, under its name.
  await bob.getByPlaceholder("Say something").fill("hi");
  await bob.getByRole("button", { name: "Say" }).click();
  for (const tab of [ann, bob]) await expect(tab.locator("#said")).toContainText("bob: hi");
  // A page closed: its connection's close block runs, and the room hears who left.
  await bob.close();
  await expect(ann.locator("#said")).toContainText("bob left");
  await ann.close();
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
    const text = readFileSync(new URL("../../crates/holon-author/samples/chat-proxy.holon.ts", import.meta.url), "utf8");
    await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
    const editor = page.getByRole("region", { name: "Text" }).locator(".cm-content");
    await editor.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text.replaceAll('"host:api.anthropic.com"', `"host:${host}"`).replace("port: 8127", `port: ${8127 + SAMPLE_SHIFT}`));
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
    // The room: a piped call's pieces to whoever watches, as they arrive, then the run's pack. The
    // watcher is subscribed once its stream opens: the server subscribes it before it sends the
    // headers, but logs GET /room/live before both, so the log can say so too soon.
    await chat.evaluate(() => {
      const w = window as unknown as { heard: string[]; open: boolean };
      w.heard = [];
      w.open = false;
      const live = new EventSource("/room/live");
      live.onopen = () => (w.open = true);
      live.onmessage = (e) => w.heard.push(e.data);
    });
    await expect.poll(() => chat.evaluate(() => (window as unknown as { open: boolean }).open)).toBe(true);
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
    const text = readFileSync(new URL("../../crates/holon-author/samples/gemini-chat.holon.ts", import.meta.url), "utf8");
    await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
    const editor = page.getByRole("region", { name: "Text" }).locator(".cm-content");
    await editor.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text.replaceAll('"host:generativelanguage.googleapis.com"', `"host:${host}"`).replace("port: 8128", `port: ${8128 + SAMPLE_SHIFT}`));
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
  await openDeployPanel(page);
  const settings = page.getByRole("complementary", { name: "Deployment settings" });
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

test("stock-sheet: a state restore's dry run names the file the runs since wrote, an edit by hand, and its diff; the restore leaves the file", async ({ page, request, adminToken }) => {
  const { rmSync, writeFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  const sheet = join(FOLDERS, "sheets", "changes.txt");
  rmSync(sheet, { force: true });
  await openSample(page, "Stock sheet (its state restored, its files not)");
  const port = await deploy(page);
  const item = `tea-${Date.now()}`;
  for (const count of [4, 40]) expect((await request.put(`http://127.0.0.1:${port}/stock/${item}`, { data: { count } })).status()).toBe(200);
  const auth = { Authorization: `Bearer ${adminToken}` };
  const log = (await (await request.get(`${SERVER}/deployments/stock-sheet/log`, { headers: auth })).json()) as { entries: { seq: number }[] };
  const [first, second] = log.entries.slice(-2).map((e) => e.seq);
  // Edited by hand after the runs: not what the last one left.
  const written = readFileSync(sheet, "utf8");
  expect(written).toBe(`${item}: 4\n${item}: 40\n`);
  writeFileSync(sheet, `${written}checked by hand\n`);

  // ↺ on the first PUT's run: its state only (its code is the code deployed now).
  await viewNewestRun(page);
  await page.locator(".hl-view__bottom").getByRole("button", { name: `restore to run ${first}`, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "dry run" });
  await expect(dialog.locator(".hl-dryrun__changes")).toContainText("files: left as they are (1 written since, 1 of them changed outside a run)");
  const files = dialog.locator(".hl-dryrun__section").filter({ hasText: "files, not put back" });
  await expect(files).toContainText(`sheets/changes.txt: 1 write since, by run ${second} (the last written); changed outside a run since run ${second}`);
  // Its diff: the file then (a later run read it) beside now, the edit by hand and all.
  const fileDiff = dialog.getByRole("region", { name: "file sheets/changes.txt" });
  await expect(fileDiff.locator(".hl-review__text")).toHaveText([`${item}: 4`, `${item}: 40`, "checked by hand"]);
  await expect(fileDiff.locator(".hl-review__row--add .hl-review__text")).toHaveText([`${item}: 40`, "checked by hand"]);
  await expect(fileDiff).toContainText(`A restore leaves it as it is now. Changed outside a run since run ${second}.`);
  const stateDiff = dialog.getByRole("region", { name: "state" });
  await expect(stateDiff.locator(".hl-review__row--del .hl-review__text")).toHaveText([`stock["${item}"]: 40`]);
  await expect(stateDiff.locator(".hl-review__row--add .hl-review__text")).toHaveText([`stock["${item}"]: 4`]);

  // Restored: the count back, the file as it was left.
  await dialog.getByRole("button", { name: "Restore", exact: true }).click();
  await expect(page.locator(".hl-history__note")).toContainText(`the state run ${first} left is live again`);
  expect(await (await request.get(`http://127.0.0.1:${port}/stock/${item}`)).json()).toEqual({ item, count: 4 });
  expect(readFileSync(sheet, "utf8")).toBe(`${written}checked by hand\n`);
});

test("balance-trail: a cell's history lists the runs that changed it and the restore between them, and opens a run", async ({ page, request, adminToken }) => {
  await openSample(page, "Balance trail (which run changed a value)");
  const port = await deploy(page);
  // Accounts of this test's own: an earlier test may have moved others.
  const account = `a-${Date.now()}`;
  const move = async (who: string, amount: number) => expect((await request.post(`http://127.0.0.1:${port}/accounts/${who}/move`, { data: { amount } })).status()).toBe(200);
  await move(account, 100);
  await move(`${account}-b`, 5);
  await move(account, -30);
  const auth = { Authorization: `Bearer ${adminToken}` };
  const log = (await (await request.get(`${SERVER}/deployments/balance-trail/log`, { headers: auth })).json()) as { entries: { seq: number }[] };
  const [first, , third] = log.entries.slice(-3).map((e) => e.seq);
  // The state the first move left put back (a person's restore), then one more move.
  expect((await request.post(`${SERVER}/deployments/balance-trail/restore`, { headers: auth, data: { run: first, state: true } })).status()).toBe(200);
  await move(account, 1);

  // View the newest run: its flow state names the account it moved, ⧗ beside it.
  await viewNewestRun(page);
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await inspector.getByRole("button", { name: /^State (at step|after the run)/ }).click();
  await inspector.getByRole("button", { name: `history of balances["${account}"]`, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "cell history" });
  await expect(dialog).toContainText(`balances["${account}"]`);
  await expect(dialog).toContainText("now 101");
  const rows = dialog.locator(".hl-cellhistory__row");
  // Newest first: the last move, the restore between, the move to 70, the first; not b's.
  await expect(rows.locator(".hl-cellhistory__values")).toHaveText(["100 → 101", "70 → 100", "100 → 70", "(none) → 100"]);
  await expect(rows.nth(1)).toContainText(`between runs ${third} and`);
  await expect(rows.nth(1)).toContainText("restore: put back the state");
  await expect(rows.nth(2)).toContainText(`POST /accounts/${account}/move · 200`);

  // A run's row opens that run: its flow state shows the move it made.
  await rows.nth(2).getByRole("button", { name: `run ${third}`, exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(inspector).toContainText("100 → 70");
});

test("rolling-log keeps only its newest 5 runs, as its deploy settings ask, while its state counts every beat", async ({ page, request, adminToken }) => {
  await openSample(page, "Rolling log (retention)");
  // Its retention opens with it, in the deployment settings: fewer runs than the host's default,
  // which asks nobody.
  await openDeployPanel(page);
  const settings = page.getByRole("complementary", { name: "Deployment settings" });
  const runs = settings.getByLabel("runs the log keeps");
  await expect(runs).toHaveValue("5");
  await expect(settings.getByLabel("days the log keeps runs")).toHaveValue("1");
  // Its text's keep line says so, and the two follow each other: the line typed, the settings show
  // it; the settings changed, the line is written again.
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  const text = page.getByRole("region", { name: "Text" }).locator(".cm-content");
  await expect(text).toContainText("keep: { runs: 5, days: 1 }");
  const source = readFileSync(new URL("../../crates/holon-author/samples/rolling-log.holon.ts", import.meta.url), "utf8");
  await text.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(source.replace("keep: { runs: 5, days: 1 }", "keep: { runs: 7, days: 1 }"));
  await expect(runs).toHaveValue("7", { timeout: 15_000 });
  await runs.fill("5");
  // Written again from the canvas.
  await expect(text).toContainText("keep: { runs: 5, days: 1 }");
  await deploy(page);
  const auth = { Authorization: `Bearer ${adminToken}` };
  const seqs = async () =>
    ((await (await request.get(`${SERVER}/deployments/rolling-log/log`, { headers: auth })).json()) as { entries: { seq: number }[] }).entries.map((e) => e.seq);
  // A beat a second: past the fifth, the oldest drop off the front, the seqs climbing on.
  await expect.poll(async () => (await seqs())[0] ?? 0, { timeout: 20_000 }).toBeGreaterThan(0);
  const kept = await seqs();
  expect(kept).toEqual([0, 1, 2, 3, 4].map((i) => kept[0] + i));
  const state = (await (await request.get(`${SERVER}/deployments/rolling-log/state`, { headers: auth })).json()) as { state: { beats: { value: number } } };
  expect(Number(state.state.beats.value), "every beat counted, kept or not").toBeGreaterThan(5);
  // The states its cut runs left are a sweep's to clear, once the log's file is cut back too (it
  // holds up to twice what it keeps, for a keep raised later), past the tenth run. A dry run
  // counts, removing nothing; all of this run's are new, so spared (a value is swept an hour
  // after it was put).
  type Swept = { dry_run: boolean; swept: { values: number }; kept: { values: number }; spared: { values: number } };
  const dryRun = async () => (await (await request.post(`${SERVER}/admin/store/sweep`, { data: { dry_run: true }, headers: auth })).json()) as Swept;
  await expect.poll(async () => (await dryRun()).spared.values, { message: "the cut runs' states, put just now", timeout: 30_000 }).toBeGreaterThan(0);
  const swept = await dryRun();
  expect(swept.dry_run).toBe(true);
  expect(swept.kept.values, "what the kept runs and the live state name").toBeGreaterThan(0);
  expect((await seqs()).length).toBe(5);
  // The admin page's storage counts first, and asks before it sweeps anything: cancelled, or
  // with nothing old enough to sweep, it removes nothing.
  await gotoPage(page, "resources");
  await resourcesTab(page, "Host and storage");
  const store = page.getByText("the store: states and values").locator("..");
  await store.getByRole("button", { name: "sweep…" }).click();
  const asks = page.getByRole("dialog");
  await expect(store.getByText(/nothing to sweep: it keeps \d+ values/).or(asks)).toBeVisible();
  if (await asks.isVisible()) {
    await expect(asks).toContainText(/Sweep the store\? It removes \d+ values/);
    await asks.getByRole("button", { name: "Cancel" }).click();
  }
  expect((await dryRun()).kept.values).toBeGreaterThan(0);
  // What the host holds: its memory and caches, each disk it keeps things on and how full, and
  // what it keeps there.
  await expect(page.getByText(/^memory: \d/)).toBeVisible();
  await expect(page.getByText(/^flow cache: \d+ flows, .* of 256 MiB/)).toBeVisible();
  await expect(page.getByText(/^disk: \d+% full: .* free of .* in use\): data, /).first()).toBeVisible();
  await expect(page.getByRole("meter").first()).toBeVisible();
  // And what each flow holds, a row each: this one among them.
  await expect(page.getByLabel("what each flow holds").getByText("rolling-log", { exact: true })).toBeVisible();
  // When the host warns its disk is full: set here; past it, the disk is warned of, with what to
  // do; floors nothing nears, and the warning goes (where it acts too: a disk past full_at is
  // warned of as well, and the default 95% may be this machine's).
  const warnAt = page.getByLabel("when the host warns its disk is full");
  const warnForm = warnAt.locator("..");
  await warnAt.fill("1000T");
  await warnForm.getByRole("button", { name: "set" }).click();
  const alert = page.getByRole("alert", { name: "what the host warns of" });
  await expect(alert).toContainText("it warns at 1000T");
  await expect(alert).toContainText("make room");
  await warnAt.fill("2K");
  await warnForm.getByRole("button", { name: "set" }).click();
  const fullAt = page.getByLabel("when the host acts on a full disk");
  await fullAt.fill("1K");
  await fullAt.locator("..").getByRole("button", { name: "set" }).click();
  await expect(alert).toBeHidden();
  // Back as they were, for the tests after: warn_at first, so full_at stays past it.
  await warnAt.fill("85%");
  await warnForm.getByRole("button", { name: "set" }).click();
  await fullAt.fill("95%");
  await fullAt.locator("..").getByRole("button", { name: "set" }).click();
  await expect(page.getByText(/^when a disk is full/)).toContainText("past 95% full");
  // What it does when a disk is full: shown, and chosen here (left as it was: only warn).
  await expect(page.getByLabel("what the host does when its disk is full")).toHaveValue("warn");
  await expect(page.getByText(/^when a disk is full/)).toContainText("it only warns");
  await expect(page.getByText(/^backup/).first()).toContainText("none: the runs logs let go of go");
  // How often the server sweeps on its own: shown here, and set here, into its holon.toml.
  const schedule = page.getByText("the server sweeps on its own").locator("..");
  await expect(schedule).toContainText("every 10m");
  const every = schedule.getByLabel("how often the server sweeps");
  await every.fill("5s");
  await expect(schedule.getByRole("button", { name: "set" }), "too often: not a setting it takes").toBeDisabled();
  await every.fill("2h");
  await schedule.getByRole("button", { name: "set" }).click();
  await expect(schedule).toContainText("every 2h");
  expect(((await (await request.get(`${SERVER}/permissions`, { headers: auth })).json()) as { storage: { sweep_every: string } }).storage.sweep_every).toBe("2h");
  await every.fill("10m");
  await schedule.getByRole("button", { name: "set" }).click();
  await expect(schedule).toContainText("every 10m");
  // Its clock would beat for the rest of the run.
  await request.post(`${SERVER}/deployments/rolling-log/stop`, { data: {}, headers: auth });
});

test("guarded-api's token rolls back under ⚙ once a dry run has said what it would do, and nothing changes until then", async ({ page, request, adminToken }) => {
  const asAdmin = { Authorization: `Bearer ${adminToken}` };
  const set = (value: string) => request.post(`${SERVER}/deployments/guarded-api/secrets/api_token`, { data: { value }, headers: asAdmin });
  expect((await set(`e2e-token-one-${Date.now()}`)).ok()).toBe(true);
  expect((await set(`e2e-token-two-${Date.now()}`)).ok()).toBe(true);
  const versions = async () =>
    ((await (await request.get(`${SERVER}/deployments/guarded-api/secrets/api_token`, { headers: asAdmin })).json()) as { versions: { version: number; current: boolean }[] }).versions;
  const [now, before] = (await versions()).map((v) => v.version);

  await openSample(page, "Token-guarded API");
  await openDeployPanel(page);
  const settings = page.getByRole("complementary", { name: "Deployment settings" });
  await settings.getByTitle("versions and changes; roll back to an older version").first().click();
  const rollBack = settings.getByTitle(`make version ${before} current again`);
  // The dry run: what it would do, never the value; cancelled, nothing changed.
  await rollBack.click();
  const dryRun = page.getByRole("dialog", { name: "dry run" });
  await expect(dryRun).toContainText(`Version ${before} (sealed here) is current again, as version ${now + 1}`);
  await expect(dryRun).toContainText(`Version ${now}, current now, is kept`);
  // What it changes, and its diff: each version by its fingerprint, as the log marks it.
  await expect(dryRun.locator(".hl-dryrun__changes")).toContainText(new RegExp(`secret api_token: v${now} #[0-9a-f]{8} → v${now + 1} #[0-9a-f]{8}, version ${before}'s value`));
  await expect(dryRun.locator(".hl-review__row--del .hl-review__text")).toHaveText([new RegExp(`^v${now} #[0-9a-f]{8}  current now`)]);
  await expect(dryRun.locator(".hl-review__row--add .hl-review__text")).toHaveText([new RegExp(`^v${now + 1} #[0-9a-f]{8}  version ${before}'s value`)]);
  await dryRun.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(dryRun).toHaveCount(0);
  expect((await versions())[0]).toEqual({ version: now, current: true, source: "local", set_at: expect.any(Number) });
  // Approved: rolled back, as a new version.
  await rollBack.click();
  await dryRun.getByRole("button", { name: "Roll back", exact: true }).click();
  await expect(settings).toContainText(`version ${before} is current again, as version ${now + 1}`);
  expect((await versions())[0]).toMatchObject({ version: now + 1, current: true });
});

test("calc's switch answers the chosen arm or 400 when it has no value, and view mode shows the other arms as not run", async ({ page, request }) => {
  await openSample(page, "Calculator (a switch)");
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
  await page.locator(".hl-view__bottom").getByRole("button", { name: "Timeline" }).click();
  await page.getByTitle("step forward one node").click();
  await expect(text.locator(".hl-text__step")).toHaveCount(1);
  // The cursor just after `a / b`, its case's answer before `?? status(400)`.
  await text.locator(".cm-line", { hasText: "return a / b" }).click();
  await page.keyboard.press("End");
  for (const _ of " ?? status(400)") await page.keyboard.press("ArrowLeft");
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
  // The whole log as a download, bigger than a file read whole: streamed by the host, as it is.
  const download = await request.get(`http://127.0.0.1:${port}/logs/${name}/download`);
  expect(download.status()).toBe(200);
  expect(download.headers()["content-disposition"]).toBe(`attachment; filename="${name}"`);
  expect(download.headers()["content-length"]).toBe(String(log.length));
  expect(await download.text()).toBe(log);
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
  expect(verified).toContain("re-ran 4 runs: each produced what its log entry says");
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
  await gotoPage(page, "resources");
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

  // The policy, read only, on a page of its own.
  await gotoPage(page, "policy");
  await expect(page.getByRole("button", { name: "read the policy again" })).toBeVisible();
  await expect(page.getByLabel("limits")).toBeVisible();
  // Who may touch the shop, under Resources: orders-api, by table, with a link to its page,
  // where its grant changes.
  await gotoPage(page, "resources");
  await resourcesTab(page, "Who may touch what");
  const shop = page.getByLabel("database shop", { exact: true });
  await expect(shop).toContainText("orders-api");
  await expect(shop).toContainText("writes orders");
  await page.getByLabel("show what").selectOption("folders");
  await expect(shop).toHaveCount(0);
  await page.getByLabel("show what").selectOption("databases");
  await shop.getByRole("button", { name: "card →" }).first().click();
  await expect(page).toHaveURL(/#orders-api\.settings$/);
  await expect(page.getByRole("region", { name: "permissions" }).getByRole("button", { name: "Revoke" }).first()).toBeInViewport();
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
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).dispatchEvent("click");
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

test("approval-mail mails a payout's approval link, its page's button approves it, and whoever asked is mailed", async ({ page, request, adminToken }) => {
  const { existsSync, readdirSync } = await import("node:fs");
  const { join } = await import("node:path");
  // What the e2e server's SMTP sink took (smtp-sink.mjs), oldest first; a mail's text, its
  // quoted-printable undone.
  const mailbox = join(FOLDERS, "mail");
  const mails = (): { from: string; to: string[]; data: string }[] =>
    existsSync(mailbox) ? readdirSync(mailbox).filter((f) => f.endsWith(".json")).sort().map((f) => JSON.parse(readFileSync(join(mailbox, f), "utf8"))) : [];
  const unquoted = (data: string) =>
    Buffer.from(data.replace(/=\r\n/g, "").replace(/=([0-9A-F]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16))), "latin1").toString("utf8");
  const mailed = async (subject: string) => {
    await expect.poll(() => mails().some((m) => m.data.includes(`\r\nSubject: ${subject}\r\n`)), { timeout: 15_000 }).toBe(true);
    return mails().find((m) => m.data.includes(`\r\nSubject: ${subject}\r\n`))!;
  };

  await openSample(page, "Approval mail (a second person approves each payout from a mail)");
  const port = await deploy(page);
  const id = `p${Date.now()}`;
  const asked = await request.post(`http://127.0.0.1:${port}/payouts`, { data: { id, amount: 5000, iban: "DE89370400440532013000", by: "sam@acme.com" } });
  expect(asked.status()).toBe(202);
  expect(await asked.json()).toEqual({ id, waiting: "approval" });

  // The approvers are mailed, from the binding's sender, the payout attached.
  const approval = await mailed(`Approve payout ${id}: 5000 to DE89370400440532013000`);
  expect([approval.from, approval.to]).toEqual(["treasury@acme.com", ["approvals@acme.com"]]);
  expect(approval.data.startsWith("From: Treasury <treasury@acme.com>\r\nTo: approvals@acme.com\r\n")).toBe(true);
  expect(approval.data).toContain('Content-Disposition: attachment; filename="payout.csv"');
  const csv = Buffer.from(`id,amount,iban,by\n${id},5000,DE89370400440532013000,sam@acme.com\n`).toString("base64");
  expect(approval.data.replace(/\r\n/g, "")).toContain(csv);
  const link = /\/approve\/\S+/.exec(unquoted(approval.data))?.[0];
  expect(link).toBeTruthy();

  // Opening the link approves nothing; its button does.
  await page.goto(`http://127.0.0.1:${port}${link}`);
  await expect(page.getByText("Approve this payout?")).toBeVisible();
  expect(JSON.stringify(await (await request.get(`http://127.0.0.1:${port}/payouts`)).json())).not.toContain(id);
  await page.getByRole("button", { name: "Approve" }).click();
  await expect(page.getByText("Approved.")).toBeVisible();

  // Recorded, and whoever asked is mailed: an address the binding's --to allows.
  const told = await mailed(`Payout ${id} approved`);
  expect(told.to).toEqual(["sam@acme.com"]);
  const recorded = await (await request.get(`http://127.0.0.1:${port}/payouts`)).json();
  expect(recorded[id]).toEqual(expect.objectContaining({ id, amount: 5000, by: "sam@acme.com", approved_via: "POST" }));
  // The deployment's timeline says what was mailed.
  const said = async () => {
    const events = await request.get(`${SERVER}/deployments/approval-mail/events`, { headers: { Authorization: `Bearer ${adminToken}` } });
    return ((await events.json()) as { events: { kind: string; what: string }[] }).events;
  };
  await expect.poll(said, { timeout: 15_000 }).toContainEqual(expect.objectContaining({ kind: "mail", what: `mailed "Payout ${id} approved" to sam@acme.com through ops` }));

  // The admin's resources show the binding, its server without a login, and who mails through
  // it; permissions, to whom.
  await page.goto("/");
  await gotoPage(page, "resources");
  await resourcesTab(page, "Mail");
  await expect(page.getByText(/^smtp:\/\/127\.0\.0\.1:\d+ \(from env:HOLON_SECRET_OPS_SMTP\) · as Treasury <treasury@acme\.com> · to \*@acme\.com$/)).toBeVisible();
  await expect(page.getByText("used by approval-mail (who may touch what says how)")).toBeVisible();
  await resourcesTab(page, "Who may touch what");
  await expect(page.getByLabel("mail ops")).toContainText("mails approvals@acme.com, addresses it computes");
});

test("remittance-mail settles an invoice from an authenticated mail, refuses one that isn't, and moves both", async ({ page, request, adminToken }) => {
  const { execFileSync } = await import("node:child_process");
  const { existsSync, mkdtempSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  // The e2e server's mailbox (imap-server.mjs): a message is an .eml file in its folder.
  const mailbox = join(FOLDERS, "mailbox");
  const id = `r${Date.now()}`;
  const invoice = `INV-${id}`;
  const remittance = (n: string, verdictBy: string, lines: string) =>
    [
      `Authentication-Results: ${verdictBy}; spf=pass smtp.mailfrom=customer.example; dkim=pass header.d=customer.example; dmarc=pass header.from=customer.example`,
      "From: Billing <billing@customer.example>",
      "To: remittances@acme.com",
      `Subject: Remittance ${n}`,
      "Date: Fri, 06 Nov 2026 10:00:00 +0000",
      `Message-ID: <${n}@customer.example>`,
      "MIME-Version: 1.0",
      'Content-Type: multipart/mixed; boundary="b1"',
      "",
      "--b1",
      "Content-Type: text/plain; charset=utf-8",
      "",
      "Paid, see attached.",
      "--b1",
      'Content-Type: text/csv; name="remit.csv"',
      'Content-Disposition: attachment; filename="remit.csv"',
      "",
      "invoice,amount",
      lines,
      "--b1--",
      "",
    ].join("\r\n");
  const said = async () => {
    const events = await request.get(`${SERVER}/deployments/remittance-mail/events`, { headers: { Authorization: `Bearer ${adminToken}` } });
    return ((await events.json()) as { events: { kind: string; what: string }[] }).events.map((e) => e.what);
  };

  await openSample(page, "Remittance mail (invoices settled from customers' mail)");
  const port = await deploy(page);
  expect((await request.post(`http://127.0.0.1:${port}/invoices`, { data: { id: invoice, amount: "100.00", customer: "customer.example" } })).status()).toBe(201);
  // Once it listens, what the mailbox held then isn't run on; what comes after is.
  await expect.poll(async () => (await said()).some((w) => w.startsWith("listens to the mailbox of remit (imap://127.0.0.1:")), { timeout: 15_000 }).toBe(true);
  writeFileSync(join(mailbox, "INBOX", `${id}.eml`), remittance(id, "mx.acme.com", `${invoice},100.00`));
  writeFileSync(join(mailbox, "INBOX", `${id}-forged.eml`), remittance(`${id}-forged`, "evil.example", `${invoice},100.00`));

  // Within a look (every 10s): paid by the authenticated one, the other kept with its verdict.
  const paid = async () => ((await (await request.get(`http://127.0.0.1:${port}/invoices`)).json()) as { paid: Record<string, { by: string }> }).paid[invoice];
  await expect.poll(paid, { timeout: 30_000 }).toEqual(expect.objectContaining({ by: "billing@customer.example", message: `${id}@customer.example` }));
  const remittances = await (await request.get(`http://127.0.0.1:${port}/remittances`)).json();
  expect(remittances[`${id}@customer.example`]).toEqual(expect.objectContaining({ settled: [invoice], unmatched: [] }));
  expect(remittances[`${id}-forged@customer.example`]).toEqual(expect.objectContaining({ refused: "not authenticated", verdict: { spf: "none", dkim: "none", dmarc: "none" } }));
  // Each moved to Processed once its run was over.
  await expect.poll(() => [`${id}.eml`, `${id}-forged.eml`].map((f) => [existsSync(join(mailbox, "INBOX", f)), existsSync(join(mailbox, "Processed", f))]), { timeout: 15_000 }).toEqual([
    [false, true],
    [false, true],
  ]);

  // Its runs check offline: each message is what its log entry says came.
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "remittance-mail.json");
  execFileSync(holon, ["export", "remittance-mail", "--server", SERVER, "-o", out], { encoding: "utf8", env: { ...process.env, HOLON_TOKEN: adminToken } });
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified).toContain("each produced what its log entry says");
  expect(verified.trim().endsWith("ok")).toBe(true);

  // The admin's resources show the mailbox, its server without a login, and whose verdict counts.
  await page.goto("/");
  await gotoPage(page, "resources");
  await resourcesTab(page, "Mail");
  await expect(page.getByText(/^receives in imap:\/\/127\.0\.0\.1:\d+ INBOX \(from env:HOLON_SECRET_REMIT_IMAP\), moved to Processed · mx\.acme\.com's verdict$/)).toBeVisible();
  await resourcesTab(page, "Who may touch what");
  await expect(page.getByLabel("mail remit")).toContainText("starts on mail it receives");
});

test("price-feed runs on each event its feed sends, and opens the feed again from where it was", async ({ page, request, adminToken }) => {
  const { existsSync, writeFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  // The e2e server's feed (sse-server.mjs): an event is a .json file dropped in its folder, its
  // name the event's id; `cut` drops every connection; connections.log says where each opened from.
  const feed = join(FOLDERS, "feed");
  const opened = () => (existsSync(join(feed, "connections.log")) ? readFileSync(join(feed, "connections.log"), "utf8").trim().split("\n") : []);
  const symbol = `S${Date.now()}`;
  const said = async () => {
    const events = await request.get(`${SERVER}/deployments/price-feed/events`, { headers: { Authorization: `Bearer ${adminToken}` } });
    return ((await events.json()) as { events: { kind: string; what: string }[] }).events.map((e) => e.what);
  };

  await openSample(page, "Price feed (server-sent events, each event a run)");
  const port = await deploy(page);
  await expect.poll(async () => (await said()).some((w) => w.startsWith("listens to the feed prices (http://127.0.0.1:")), { timeout: 15_000 }).toBe(true);
  // Its key never shown.
  expect((await said()).some((w) => w.includes("e2e-not-a-key"))).toBe(false);
  writeFileSync(join(feed, `${symbol}-1.json`), JSON.stringify({ symbol, price: 10.5 }));
  type Price = { price: number; at: number };
  const prices = async () => (await (await request.get(`http://127.0.0.1:${port}/prices`)).json()) as Record<string, Price>;
  await expect.poll(async () => (await prices())[symbol]?.price, { timeout: 15_000 }).toBe(10.5);
  // The server panel says what its connection is doing, and where it is.
  await expect(page.getByText(`connected, at ${symbol}-1`)).toBeVisible({ timeout: 15_000 });

  // Every connection dropped: opened again from the last event run on, and what comes is run on.
  const before = opened().length;
  writeFileSync(join(feed, "cut"), "");
  await expect.poll(() => opened().slice(before), { timeout: 15_000 }).toEqual([`${symbol}-1`]);
  writeFileSync(join(feed, `${symbol}-2.json`), JSON.stringify({ symbol, price: 11 }));
  await expect.poll(async () => (await prices())[symbol]?.price, { timeout: 15_000 }).toBe(11);

  // The admin's resources show the feed without its query; permissions, that price-feed listens.
  await page.goto("/");
  await gotoPage(page, "resources");
  await resourcesTab(page, "Feeds");
  await expect(page.getByText(/^http:\/\/127\.0\.0\.1:\d+\/events \(from env:HOLON_SECRET_PRICES_URL\) · 1000 may wait · quiet for 90s at most$/)).toBeVisible();
  await expect(page.getByText("e2e-not-a-key")).toHaveCount(0);
  await resourcesTab(page, "Who may touch what");
  await expect(page.getByLabel("feed prices")).toContainText("starts on each event it sends");
});

test("payout-events runs each message its queue holds once, and sends a failed payout's retry to the FIFO queue", async ({ page, request, adminToken }) => {
  const { existsSync, writeFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  // The e2e server's SQS (sqs-server.mjs): a message is a file dropped in its queue's folder;
  // sent.log and deleted.log say what the host sent and deleted.
  const sqs = join(FOLDERS, "sqs");
  type Line = { queue: string; id: string; body: string; group?: string | null };
  const lines = (file: string): Line[] =>
    existsSync(join(sqs, file))
      ? readFileSync(join(sqs, file), "utf8")
          .trim()
          .split("\n")
          .filter(Boolean)
          .map((l) => JSON.parse(l) as Line)
      : [];
  const stamp = Date.now();
  const id = (n: number) => `po_${stamp}_${n}`;
  const event = (type: string, n: number) => JSON.stringify({ type, payout: { id: id(n), amount: "120.00", currency: "EUR", account: `acct_${stamp}` } });
  const said = async () => {
    const events = await request.get(`${SERVER}/deployments/payout-events/events`, { headers: { Authorization: `Bearer ${adminToken}` } });
    return ((await events.json()) as { events: { kind: string; what: string }[] }).events.map((e) => e.what);
  };

  await openSample(page, "Payout events (an SQS queue in, a FIFO queue of retries out)");
  const port = await deploy(page);
  await expect.poll(async () => (await said()).some((w) => w.startsWith("listens to the queue payout_events (http://127.0.0.1:")), { timeout: 15_000 }).toBe(true);
  // Its key never shown.
  expect((await said()).some((w) => w.includes("e2e-not-a-secret"))).toBe(false);
  writeFileSync(join(sqs, "payout-events", `${stamp}-1.json`), event("payout.paid", 1));
  writeFileSync(join(sqs, "payout-events", `${stamp}-2.json`), event("payout.failed", 2));
  type Payout = { status: string; event: string; account: string };
  const payouts = async () => (await (await request.get(`http://127.0.0.1:${port}/payouts`)).json()) as Record<string, Payout>;
  await expect.poll(async () => [(await payouts())[id(1)]?.status, (await payouts())[id(2)]?.status], { timeout: 15_000 }).toEqual(["paid", "failed"]);
  // Each deleted once its run was over; the failed one's retry in the FIFO queue, its group the
  // account, sent once.
  await expect.poll(() => lines("deleted.log").filter((d) => d.body.includes(String(stamp))).length, { timeout: 15_000 }).toBe(2);
  await expect.poll(() => lines("sent.log").filter((s) => s.body.includes(String(stamp))).map((s) => [s.queue, JSON.parse(s.body).id, s.group]), { timeout: 15_000 }).toEqual([
    ["payout-retries.fifo", id(2), `acct_${stamp}`],
  ]);
  // The server panel says what the listener did and what was sent.
  await expect(page.getByText("receiving, 2 run")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText("1 sent")).toBeVisible({ timeout: 15_000 });

  // The admin's resources show each queue without its key; permissions, who takes and who sends.
  await page.goto("/");
  await gotoPage(page, "resources");
  await resourcesTab(page, "Queues");
  await expect(page.getByText(/^http:\/\/127\.0\.0\.1:\d+\/000000000000\/payout-retries\.fifo \(from env:HOLON_SECRET_RETRIES_URL\) · FIFO · 256 KiB at most · out of sight for 300s$/)).toBeVisible();
  await expect(page.getByText("e2e-not-a-secret")).toHaveCount(0);
  await resourcesTab(page, "Who may touch what");
  await expect(page.getByLabel("queue payout_events")).toContainText("starts on each message it holds");
  await expect(page.getByLabel("queue payout_retries")).toContainText("sends messages to it");
});

test("ledger-watch keeps each account's balance from its ledger's channel, and reads what it didn't hear while stopped", async ({ page, request, adminToken }) => {
  test.skip(!process.env.HOLON_TEST_POSTGRES, "set HOLON_TEST_POSTGRES (a Postgres's URL) to run it against Postgres");
  const { execFileSync } = await import("node:child_process");
  const { mkdtempSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const env = { ...process.env, HOLON_TOKEN: adminToken };
  const auth = { Authorization: `Bearer ${adminToken}` };
  const said = async () => ((await (await request.get(`${SERVER}/deployments/ledger-watch/events`, { headers: auth })).json()) as { events: { what: string }[] }).events.map((e) => e.what);
  const feed = async () => ((await (await request.get(`${SERVER}/deployments/ledger-watch/feeds`, { headers: auth })).json()) as { feeds: { status: string; address?: string }[] }).feeds[0];

  // The service that writes the ledger, standing in for one: a flow of the test's own, the ledger
  // bound read-write for it alone (server.mjs).
  const writerPort = 8199 + SAMPLE_SHIFT;
  const writer = join(mkdtempSync(join(tmpdir(), "holon-ledger-writer-")), "ledger-writer.holon.ts");
  writeFileSync(
    writer,
    `import { flow, route } from "holon"\nimport ledger from "database:ledger"\n\nflow("ledger-writer", { port: ${writerPort} })\n\nroute.post("/entries", (req) => {\n  ledger.exec("insert into entries (account, seq, amount) values ($1, $2, $3)", req.json.account, req.json.seq, req.json.amount)\n  return { written: req.json.seq }\n})\n`,
  );
  execFileSync(holon, ["deploy", writer, "--server", SERVER, "--slug", "ledger-writer", "--approve"], { encoding: "utf8", env });
  const account = `acct_${Date.now()}`;
  const write = async (seq: number, amount: string) => expect((await request.post(`http://127.0.0.1:${writerPort}/entries`, { data: { account, seq, amount } })).status()).toBe(200);

  await openSample(page, LEDGER_WATCH);
  const port = await deploy(page);
  await expect.poll(async () => (await feed())?.status, { timeout: 15_000 }).toBe("connected");
  expect((await feed()).address).toMatch(/^postgres:\/\/127\.0\.0\.1:\d+\/\w+, channel ledger_entries$/);
  type Balance = { balance: string; entries: number; read: number; last: number };
  const balance = async () => ((await (await request.get(`http://127.0.0.1:${port}/balances`)).json()) as Record<string, Balance>)[account];
  await write(1, "120.00");
  await write(2, "-20.50");
  await expect.poll(balance, { timeout: 15_000 }).toEqual({ balance: "99.50", entries: 2, read: 0, last: 2 });

  // Stopped, its session closed: entry 3 is never sent. Run again, entry 4's run reads it from the
  // ledger.
  expect((await request.post(`${SERVER}/deployments/ledger-watch/stop`, { data: {}, headers: auth })).ok()).toBe(true);
  await expect.poll(async () => (await feed())?.status, { timeout: 15_000 }).toBe("closed");
  await write(3, "10.00");
  expect((await request.post(`${SERVER}/deployments/ledger-watch/run`, { data: {}, headers: auth })).ok()).toBe(true);
  await expect.poll(async () => (await feed())?.status, { timeout: 15_000 }).toBe("connected");
  await write(4, "2.25");
  await expect.poll(balance, { timeout: 15_000 }).toEqual({ balance: "111.75", entries: 4, read: 1, last: 4 });
  expect((await said()).some((w) => w.startsWith("listens to the feed entries (postgres://127.0.0.1:"))).toBe(true);
  // Its login never shown.
  const login = `:${new URL(process.env.HOLON_TEST_POSTGRES ?? "").password}@`;
  expect((await said()).some((w) => w.includes(login))).toBe(false);

  // Its runs check offline, the ledger read as each run read it.
  const out = join(mkdtempSync(join(tmpdir(), "holon-runs-")), "ledger-watch.json");
  execFileSync(holon, ["export", "ledger-watch", "--server", SERVER, "-o", out], { encoding: "utf8", env });
  const verified = execFileSync(holon, ["verify", out], { encoding: "utf8" });
  expect(verified.trim().endsWith("ok")).toBe(true);

  // The admin's resources show the feed on its channel, without the connection.
  await page.goto("/");
  await gotoPage(page, "resources");
  await resourcesTab(page, "Feeds");
  await expect(page.getByText(/^listens on postgres:\/\/127\.0\.0\.1:\d+\/\w+, channel ledger_entries \(the database ledger\)/)).toBeVisible();
  await expect(page.getByText(login)).toHaveCount(0);
});

test("traced-payout's log lines show on its Logs tab, filtered by level, and up to a run on the run's page; agents read them once let", async ({ page, request, adminToken }) => {
  await openSample(page, "Traced payout (log lines, kept apart from the runs)");
  // Its deploy settings let agents read its lines: a grant a person approves on the consent screen.
  const port = await deploy(page, SAMPLE_SHIFT, (settings) => settings.getByLabel("agents may read its logs").check());
  const payout = (id: string, amount: number) => request.post(`http://127.0.0.1:${port}/payout`, { data: { id, amount } });
  expect((await payout("p-1", 250)).status()).toBe(200);
  expect((await payout("p-2", 5000)).status()).toBe(409);

  // Its Logs tab: every line, each with its level and its run.
  await page.goto("/#traced-payout.logs");
  const lines = page.getByRole("log", { name: "lines" });
  await expect(lines).toContainText("payout asked p-1 250");
  await expect(lines).toContainText("paying p-1");
  await expect(lines).toContainText("over the usual limit p-2");
  await expect(lines).not.toContainText("paying p-2");
  // Warnings only.
  const levels = page.getByRole("group", { name: "level" });
  await levels.getByRole("button", { name: "Warnings" }).click();
  await expect(lines).toContainText("over the usual limit p-2");
  await expect(lines).not.toContainText("payout asked");

  // A line names its run, and the run's page shows the lines up to its end, and its own on asking.
  await lines.getByRole("link").first().click();
  await expect(page.getByRole("article", { name: /^run \d+$/ })).toBeVisible();
  const upTo = page.getByRole("log", { name: "lines" });
  await expect(upTo).toContainText("payout asked p-1 250");
  await expect(upTo).toContainText("over the usual limit p-2");
  await page.getByLabel(/only run #\d+'s/).check();
  await expect(upTo).toContainText("over the usual limit p-2");
  await expect(upTo).not.toContainText("p-1");
  // Let in its deploy settings, an agent (as holon mcp asks) reads them too, filtered as it asks.
  const asAgent = { Authorization: `Bearer ${adminToken}`, "X-Holon-Role": "agent" };
  const agent = await request.get(`${SERVER}/deployments/traced-payout/logs?level=warn`, { headers: asAgent });
  expect(agent.status()).toBe(200);
  expect(((await agent.json()) as { lines: { text: string }[] }).lines.map((l) => l.text)).toEqual(["over the usual limit p-2"]);
  // At phone width the run's lines still fit across.
  const wide = page.viewportSize()!;
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.setViewportSize(wide);
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
  await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).dispatchEvent("click");
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
  await gotoPage(page, "resources");
  await resourcesTab(page, "Folders");
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

  // Exported from the run log: the head it chains up to, and the file (the run pane's top bar).
  const chain = page.getByRole("region", { name: "run" });
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
  // And each began from the balances the one before left (the first, from what no run here left).
  expect(verified).toContain("state: 3 runs began from the state the run before them left");
  expect(verified).not.toContain("DOESN'T FOLLOW");
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
  // Its events with its runs: its deploys (as many as tests on this server deployed it).
  expect(asMe).toContain(`wrote ${mine}: 4 runs of "ledger" and its `);
  expect(asMe).toMatch(new RegExp(` and its \\d+ events?, head ${head.slice(0, 12)}`));
  expect(asMe).toContain(`holon verify ${mine}`);
  const checked = execFileSync(holon, ["verify", mine], { encoding: "utf8" });
  expect(checked).toContain("re-ran 4 runs");
  expect(checked).toMatch(/events: \d+, chain unbroken from the first/);

  // The balances the first posting left put back, and one more: it began from what was put back.
  const first = (JSON.parse(JSON.parse(readFileSync(mine, "utf8")).lines[0]) as { seq: number }).seq;
  const put = await request.post(`${SERVER}/deployments/ledger/restore`, { data: { run: first, state: true }, headers: { Authorization: `Bearer ${adminToken}` } });
  expect(put.status()).toBe(200);
  const posted = await request.post(`http://127.0.0.1:${port}/accounts/ada`, { data: { amount: 1 } });
  expect(((await posted.json()) as { balance: number }).balance).toBe(6);
  const again = join(dir, "restored.json");
  execFileSync(holon, ["export", "ledger", "--server", SERVER, "-o", again], { encoding: "utf8", env: { ...process.env, HOLON_TOKEN: adminToken } });
  const restored = execFileSync(holon, ["verify", again], { encoding: "utf8" });
  expect(restored).toContain("(1 of them after a restore, from the state it put back)");
  expect(restored.trim().endsWith("ok")).toBe(true);
});

test("site serves a built app from its folder: a deep link opens its page, its files as they are, a cached copy a 304, a missing asset a 404", async ({ page, request }) => {
  const { mkdirSync, writeFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  // The app's build, as a bundler leaves it: its page, its script, an image.
  const site = join(FOLDERS, "site");
  mkdirSync(join(site, "assets"), { recursive: true });
  const index = `<!doctype html><meta charset="utf-8"><title>Site</title><p>Opened at <code></code>.</p><p id="api">Asking the API…</p><script type="module" src="/assets/app.js"></script>`;
  writeFileSync(join(site, "index.html"), index);
  const script = `document.querySelector("code").textContent = location.pathname\nfetch("/api/hello").then((r) => r.json()).then((j) => { document.getElementById("api").textContent = "The API says hello, " + j.hello + "." })\n`;
  writeFileSync(join(site, "assets", "app.js"), script);
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0xfe]);
  writeFileSync(join(site, "logo.png"), png);
  await openSample(page, "Site (a built web app and its API on one port)");
  const port = await deploy(page);
  const at = (path: string, headers: Record<string, string> = {}) => request.get(`http://127.0.0.1:${port}${path}`, { headers });
  const deep = await at("/docs/a%20guide/part%202");
  expect(deep.status()).toBe(200);
  expect(deep.headers()["content-type"]).toBe("text/html; charset=utf-8");
  expect(await deep.text()).toBe(index);
  // Its etag is its hash (BLAKE3).
  const etag = deep.headers()["etag"];
  expect(etag).toMatch(/^"[0-9a-f]{64}"$/);
  expect(await (await at("/")).text()).toBe(index);
  const js = await at("/assets/app.js");
  expect(js.headers()["content-type"]).toBe("text/javascript; charset=utf-8");
  expect(js.headers()["cache-control"]).toBe("public, max-age=31536000, immutable");
  expect(await js.text()).toBe(script);
  const logo = await at("/logo.png");
  expect(logo.headers()["content-type"]).toBe("image/png");
  expect(Buffer.compare(await logo.body(), png)).toBe(0);
  // The browser's cached copy, still the file's: a 304 and no body.
  expect((await at("/docs/setup", { "if-none-match": etag })).status()).toBe(304);
  expect((await at("/docs/setup", { "if-none-match": '"stale"' })).status()).toBe(200);
  const missing = await at("/assets/nope.js");
  expect(missing.status()).toBe(404);
  expect(await missing.json()).toEqual({ error: "not found" });
  expect(await (await at("/api/hello")).json()).toEqual({ hello: "world" });
  const unknown = await at("/api/nope/more");
  expect(unknown.status()).toBe(404);
  expect(await unknown.json()).toEqual({ error: "no API at /api/nope/more" });
  // A rest never names another path than the request's: no route takes these. (A client resolves
  // `..` before it sends, so it isn't asked here; the server test asks it.)
  for (const not of ["/docs/a%2Fb", "/docs/a%5Cb", "/docs//a", "/docs/"]) {
    const answer = await at(not);
    expect(answer.status(), not).toBe(404);
    expect(await answer.json(), not).toEqual({ error: "no route for this path" });
  }
  // In a browser, a deep link opens the app where it points, its script runs, and it asks its API.
  await page.goto(`http://127.0.0.1:${port}/docs/setup`);
  await expect(page.locator("code")).toHaveText("/docs/setup");
  await expect(page.locator("#api")).toHaveText("The API says hello, world.");
});
