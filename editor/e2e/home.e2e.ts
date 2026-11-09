// The welcome and the overview (WelcomePage.tsx, HomePage.tsx), kept with the account (home.ts,
// holon-server's /auth/prefs): the welcome opens on start until told not to, and the overview's
// panes, as customized, come back after a reload. And the editor's tabs (CanvasPanels.tsx,
// EditorHome.tsx): its Home, a tab per flow, + and ×, and the panes its bar shows or hides.

import type { APIRequestContext } from "@playwright/test";
import { expect, SERVER, test } from "./fixtures";
import { gotoPage, navLink, openSample } from "./helpers";

/** Wait until the server holds what was just changed (the editor saves as it changes), so a
 *  reload reads it back rather than racing the last save. */
async function saved(request: APIRequestContext, token: string, holds: (prefs: string) => boolean) {
  const headers = { Authorization: `Bearer ${token}` };
  await expect.poll(async () => holds(await (await request.get(`${SERVER}/auth/prefs`, { headers })).text())).toBe(true);
}

test("the welcome opens on start until told not to; home keeps its panes", async ({ page, request, adminToken }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/#start$/);
  await expect(navLink(page, "Getting started")).toHaveAttribute("aria-current", "page");
  await expect(page.getByText(/^Welcome to Holon, e2e$/)).toBeVisible();
  await page.getByRole("button", { name: "▸ Secrets" }).click();
  await expect(page.getByText(/Values are sealed on the server/)).toBeVisible();

  // Not on start any more: the next start is at the overview.
  await page.getByLabel("show this on start").uncheck();
  await saved(request, adminToken, (p) => p.includes('"welcome":false'));
  // A start, not a reload: a reload keeps the place the address names (the welcome, #start).
  await page.goto("/");
  await expect(navLink(page, "Overview")).toHaveAttribute("aria-current", "page");
  await expect(page).not.toHaveURL(/#start$/);

  // Customize: a notes pane, written in, moved first; a pane removed.
  await page.getByRole("button", { name: "customize" }).click();
  await page.getByLabel("add a pane").selectOption("notes");
  const notes = page.getByRole("region", { name: "notes" });
  await notes.getByRole("textbox").fill("deploy the shop on friday");
  await page.getByRole("button", { name: "move notes earlier" }).click();
  await page.getByRole("button", { name: "remove samples" }).click();
  await page.getByRole("button", { name: "done" }).click();
  await saved(request, adminToken, (p) => p.includes("deploy the shop on friday") && !p.includes('"samples"'));
  await page.reload();
  await expect(page.getByRole("region", { name: "notes" }).getByRole("textbox")).toHaveValue("deploy the shop on friday");
  await expect(page.getByRole("region", { name: "samples" })).toHaveCount(0);

  // The welcome is always one click away, under the account menu.
  await page.getByRole("button", { name: "account: e2e" }).click();
  await page.getByRole("menuitem", { name: "welcome and docs" }).click();
  await expect(navLink(page, "Getting started")).toHaveAttribute("aria-current", "page");
  await page.getByLabel("show this on start").check();
  await saved(request, adminToken, (p) => !p.includes('"welcome":false'));
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
  // + starts a draft, with the palette; New asks nothing: an untitled draft, named on save.
  await tabs.getByRole("button", { name: "new tab" }).click();
  await expect(page.locator('[aria-modal="true"]')).toHaveCount(0);
  await expect(page.locator(".hl-tab--active")).toContainText("untitled");
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
  const closed = page.getByRole("region", { name: "recently closed" });
  await expect(closed).toContainText("with its unsaved changes");
  await closed.getByRole("button", { name: "reopen hello-api" }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("hello-api");
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
