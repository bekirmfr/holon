// The welcome and home tabs (WelcomePage.tsx, HomePage.tsx), kept with the account (home.ts,
// holon-server's /auth/prefs): the welcome opens on start until told not to, and the home tab's
// panes, as customized, come back after a reload.

import { expect, test } from "./fixtures";

test("the welcome opens on start until told not to; home keeps its panes", async ({ page }) => {
  await page.goto("/");
  const tabs = page.getByRole("tablist");
  await expect(tabs.getByRole("tab", { name: "welcome" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText(/^Welcome to Holon, e2e$/)).toBeVisible();
  await page.getByRole("button", { name: "▸ Secrets" }).click();
  await expect(page.getByText(/Values are sealed on the server/)).toBeVisible();

  // Not on start any more: the next start is at home, without the welcome tab.
  await page.getByLabel("show this on start").uncheck();
  await page.reload();
  await expect(tabs.getByRole("tab", { name: "home" })).toHaveAttribute("aria-selected", "true");
  await expect(tabs.getByRole("tab", { name: "welcome" })).toHaveCount(0);

  // Customize: a notes pane, written in, moved first; a pane removed.
  await page.getByRole("button", { name: "customize" }).click();
  await page.getByLabel("add a pane").selectOption("notes");
  const notes = page.getByRole("region", { name: "notes" });
  await notes.getByRole("textbox").fill("deploy the shop on friday");
  await page.getByRole("button", { name: "move notes earlier" }).click();
  await page.getByRole("button", { name: "remove samples" }).click();
  await page.getByRole("button", { name: "done" }).click();
  await page.reload();
  await expect(page.getByRole("region", { name: "notes" }).getByRole("textbox")).toHaveValue("deploy the shop on friday");
  await expect(page.getByRole("region", { name: "samples" })).toHaveCount(0);

  // The welcome is always one click away, under the account menu.
  await page.getByRole("button", { name: "e2e ▾" }).click();
  await page.getByRole("menuitem", { name: "welcome and docs" }).click();
  await expect(tabs.getByRole("tab", { name: "welcome" })).toHaveAttribute("aria-selected", "true");
  await page.getByLabel("show this on start").check();
});
