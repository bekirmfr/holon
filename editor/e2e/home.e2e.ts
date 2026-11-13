// Getting started and the overview (WelcomePage.tsx, OverviewPage.tsx), kept with the account (home.ts,
// holon-server's /auth/prefs): the welcome opens on start until told not to, and the notes written
// there come back after a reload; its commands copy, and the Library's tutorials open their
// samples (guide.ts, tutorials.ts). And the editor's tabs (CanvasPanels.tsx,
// EditorHome.tsx): its Home, a tab per flow, + and ×, and the panes its bar shows or hides.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { APIRequestContext } from "@playwright/test";
import { expect, HOLON, SAMPLE_SHIFT, SAMPLES, SERVER, test } from "./fixtures";
import { gotoPage, navLink, nodesLevel, openSample } from "./helpers";

/** Wait until the server holds what was just changed (the editor saves as it changes), so a
 *  reload reads it back rather than racing the last save. */
async function saved(request: APIRequestContext, token: string, holds: (prefs: string) => boolean) {
  const headers = { Authorization: `Bearer ${token}` };
  await expect.poll(async () => holds(await (await request.get(`${SERVER}/auth/prefs`, { headers })).text())).toBe(true);
}

/** Something deployed on the server: hello-api, deployed from its text by `holon deploy`, when
 *  nothing is yet. */
async function deployedOnce(request: APIRequestContext, token: string) {
  const headers = { Authorization: `Bearer ${token}` };
  const { deployments } = (await (await request.get(`${SERVER}/deployments`, { headers })).json()) as { deployments: unknown[] };
  if (deployments.length > 0) return;
  const text = readFileSync(join(SAMPLES, "hello-api.holon.ts"), "utf8").replace(/(port: )8101\b/, `$1${8101 + SAMPLE_SHIFT}`);
  const copy = join(mkdtempSync(join(tmpdir(), "holon-e2e-welcome-")), "hello-api.holon.ts");
  writeFileSync(copy, text);
  execFileSync(HOLON, ["deploy", copy, "--server", SERVER, "--slug", "hello-api", "--approve"], { env: { ...process.env, HOLON_TOKEN: token }, stdio: "pipe" });
}

test("the welcome opens on start until told not to, and keeps the notes written there", async ({ page, request, adminToken }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/#start$/);
  await expect(navLink(page, "Getting started")).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { name: "Getting started" })).toBeVisible();
  await expect(page.getByText(/^Welcome to Holon, e2e\./)).toBeVisible();
  await page.getByRole("button", { name: "▸ Secrets" }).click();
  await expect(page.getByText(/Values are sealed on the server/)).toBeVisible();

  // Not on start any more: the next start is at the overview.
  await page.getByLabel("show this on start").uncheck();
  await saved(request, adminToken, (p) => p.includes('"welcome":false'));
  // A start, not a reload: a reload keeps the place the address names (the welcome, #start).
  await page.goto("/");
  await expect(navLink(page, "Overview")).toHaveAttribute("aria-current", "page");
  await expect(page).not.toHaveURL(/#start$/);
  await expect(page.getByRole("region", { name: "at a glance" })).toBeVisible();

  // Getting started is always one click away, in the account dialog; its notes are kept there.
  await page.getByRole("button", { name: "account: e2e" }).click();
  await page.getByRole("dialog", { name: "account" }).getByRole("button", { name: "Getting started" }).click();
  await expect(navLink(page, "Getting started")).toHaveAttribute("aria-current", "page");
  const notes = page.getByLabel("notes", { exact: true });
  await notes.fill("deploy the shop on friday");
  await notes.blur();
  await saved(request, adminToken, (p) => p.includes("deploy the shop on friday"));
  await page.reload();
  await expect(page.getByLabel("notes", { exact: true })).toHaveValue("deploy the shop on friday");
  // A step done is ticked: "Deploy it", once something is deployed (by the tests before this one,
  // else here, as its command does, on the port the tests move samples to).
  await deployedOnce(request, adminToken);
  await page.reload();
  await expect(page.getByRole("listitem", { name: "Deploy it, done" })).toBeVisible();
  await page.getByLabel("show this on start").check();
  await saved(request, adminToken, (p) => !p.includes('"welcome":false'));
});

test("Help: a step's command copies as typed, and a tutorial opens the samples it uses as unsaved copies", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/#start");
  const line = "holon new hello-api.holon.ts --sample hello-api";
  const commands = page.getByRole("list", { name: "Open a sample, from a terminal" });
  await expect(commands).toContainText(line);
  await commands.getByRole("button", { name: `copy ${line}` }).click();
  await expect(commands.getByRole("button", { name: `copy ${line}` })).toHaveText("copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(line);

  // A tutorial: its page's first paragraph, its samples, and a link to it in Docs.
  await page.goto("/#library.tutorials");
  const card = page.getByRole("region", { name: "tutorials" }).getByRole("article", { name: "Stripe to Slack" });
  await expect(card).toContainText("This guide runs a real webhook end to end");
  await card.getByRole("button", { name: "open Stripe → Slack" }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("stripe-to-slack");
});

test("a welcome that loads late doesn't take you from a page you opened", async ({ page }) => {
  // The prefs answer is held until the flows page is open, as on a slow server. It is answered
  // here, as nothing kept yet (so the welcome opens on start): a held request let through to the
  // server once never reached it, and the server holds what the test before left.
  let release!: () => void;
  const held = new Promise<void>((r) => (release = r));
  await page.route("**/auth/prefs", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    await held;
    await route.fulfill({ json: {} });
  });
  await page.goto("/");
  await gotoPage(page, "flows");
  await expect(navLink(page, "Flows")).toHaveAttribute("aria-current", "page");
  const answered = page.waitForResponse("**/auth/prefs");
  release();
  // The welcome would open, but the flows page stays shown.
  await answered;
  await expect(page).toHaveURL(/#flows$/);
  await expect(navLink(page, "Flows")).toHaveAttribute("aria-current", "page");
});

test("the editor's tabs: Home, a tab per flow, + and ×; a closed tab opens again from Home", async ({ page }) => {
  await page.goto("/#editor");
  const tabs = page.getByRole("tablist", { name: "open tabs" });
  // No flow open: the editor shows its Home, its only tab.
  await expect(tabs.getByRole("tab")).toHaveText([/^Home/]);
  await expect(tabs.getByRole("tab", { name: "Home" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("region", { name: "open a flow" })).toBeVisible();
  await expect(page.getByRole("region", { name: "recently closed" })).toContainText("no tab closed yet");
  // + starts a draft, with the palette at its Nodes level; New asks nothing: an untitled draft,
  // named on save.
  await tabs.getByRole("button", { name: "new tab" }).click();
  await expect(page.locator('[aria-modal="true"]')).toHaveCount(0);
  await expect(page.locator(".hl-tab--active")).toContainText("untitled");
  await nodesLevel(page);
  await expect(page.locator(".hl-work .hl-palette")).toBeVisible();
  // A sample takes the empty draft's tab.
  await openSample(page, "Hello API", false);
  await expect(tabs.getByRole("tab")).toHaveText([/^Home/, /^hello-api/]);
  // Home again, over the canvas: the flow's tab stays open.
  await tabs.getByRole("tab", { name: "Home" }).click();
  await expect(page).toHaveURL(/#editor\.home$/);
  await expect(page.getByRole("region", { name: "start new" })).toBeVisible();
  await expect(page.locator(".hl-palette")).toHaveCount(0);
  await tabs.getByRole("tab", { name: /^hello-api/ }).click();
  await expect(page).toHaveURL(/#editor\.hello-api$/);
  // Closed (its changes discarded, asked first), it is among the recently closed, and opens
  // again as it was left.
  await tabs.getByRole("button", { name: "close hello-api" }).click();
  await page.locator('[aria-modal="true"]').getByRole("button", { name: "Discard" }).click();
  await expect(tabs.getByRole("tab")).toHaveText([/^Home/]);
  // The last flow closed, the address is the editor's Home, not the flow's.
  await expect(page).toHaveURL(/#editor\.home$/);
  const closed = page.getByRole("region", { name: "recently closed" });
  await expect(closed).toContainText("with its unsaved changes");
  await closed.getByRole("button", { name: "reopen hello-api" }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("hello-api");
  await expect(page).toHaveURL(/#editor\.hello-api$/);
  await nodesLevel(page);
  await expect(page.locator(".hl-canvas__flow > .react-flow .react-flow__node").first()).toBeVisible();
  // The editor's tabs are the editor's: a page has none.
  await gotoPage(page, "resources");
  await expect(tabs).toHaveCount(0);
});

test("the bar's panes: c, x and i show or hide the canvas, the text and the Inspector, kept with the account", async ({ page, request, adminToken }) => {
  await openSample(page, "Hello API");
  const canvas = page.locator(".hl-canvas");
  const panes = canvas.getByRole("group", { name: "panes" });
  await expect(panes.getByRole("button", { name: "inspector on" })).toBeVisible();
  await page.locator(".hl-canvas__flow").click({ position: { x: 5, y: 5 } });
  // i folds the Inspector; c hides the canvas, and the text takes its room.
  await page.keyboard.press("i");
  await expect(page.getByRole("complementary", { name: "Inspector" })).toHaveCount(0);
  await expect(panes.getByRole("button", { name: "inspector off" })).toBeVisible();
  await page.keyboard.press("c");
  await expect(page.getByRole("region", { name: "Text" })).toBeVisible();
  await expect(page.locator(".hl-canvas__flow")).toBeHidden();
  await saved(request, adminToken, (p) => p.includes('"hidden":["inspector","canvas"]'));
  // Kept over a reload, as the account's.
  await page.reload();
  await openSample(page, "Hello API", false);
  await expect(page.getByRole("complementary", { name: "Inspector" })).toHaveCount(0);
  // Closing the text brings the canvas back: one of them always shows.
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  await expect(page.locator(".hl-canvas__flow")).toBeHidden();
  await panes.getByRole("button", { name: "text on" }).click();
  await expect(page.locator(".hl-canvas__flow")).toBeVisible();
  await panes.getByRole("button", { name: "inspector off" }).click();
  await expect(page.getByRole("complementary", { name: "Inspector" })).toBeVisible();
  await saved(request, adminToken, (p) => p.includes('"hidden":[]'));
});

test("closing a changed tab asks: Save and close saves it, then closes it", async ({ page, request, adminToken }) => {
  await openSample(page, "Hello API");
  const tabs = page.getByRole("tablist", { name: "open tabs" });
  // A route of its own, typed in the text: once the canvas shows it, the text stands for the flow
  // and is what a save keeps.
  const path = `/saved-on-close-${Date.now()}/:name`;
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).dispatchEvent("click");
  const line = page.getByRole("region", { name: "Text" }).locator(".cm-line", { hasText: 'route.get("/hello/:name"' });
  await line.click();
  await page.keyboard.press("Home");
  await page.keyboard.press("Shift+End");
  await page.keyboard.insertText(`route.get("${path}", (req) => \`Hello, \${req.params.name}\`)`);
  await expect(page.locator(".hl-canvas__flow")).toContainText(path);
  await expect(page.locator(".hl-bar__state--changed")).toHaveText("changed");
  await tabs.getByRole("button", { name: "close hello-api" }).click();
  const dialog = page.locator('[aria-modal="true"]');
  await expect(dialog).toContainText('"hello-api" has unsaved changes.');
  await expect(dialog.getByRole("button")).toHaveText(["kKeep it open", "dDiscard", "sSave and close"]);
  // Keep it open (k): nothing changes.
  await page.keyboard.press("k");
  await expect(dialog).toHaveCount(0);
  await expect(tabs.getByRole("tab")).toHaveText([/^Home/, /^hello-api/]);
  // Save and close: saved on the server with the change, closed, and reopened as saved.
  await tabs.getByRole("button", { name: "close hello-api" }).click();
  await dialog.getByRole("button", { name: "Save and close" }).click();
  await expect(tabs.getByRole("tab")).toHaveText([/^Home/]);
  const saved = await request.get(`${SERVER}/flows/hello-api`, { headers: { Authorization: `Bearer ${adminToken}` } });
  expect(((await saved.json()) as { document: { source?: string } }).document.source).toContain(path);
  const closed = page.getByRole("region", { name: "recently closed" });
  await expect(closed).not.toContainText("with its unsaved changes");
});
