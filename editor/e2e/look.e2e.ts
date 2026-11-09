// How the editor looks (look.ts): auto follows the system, light and dark are chosen in the top
// bar, kept with the account, and the text and canvas follow.

import type { APIRequestContext } from "@playwright/test";
import { expect, SERVER, test } from "./fixtures";
import { openSample } from "./helpers";

async function savedTheme(request: APIRequestContext, token: string, theme: string) {
  const headers = { Authorization: `Bearer ${token}` };
  await expect.poll(async () => (await (await request.get(`${SERVER}/auth/prefs`, { headers })).text()).includes(`"theme":"${theme}"`)).toBe(true);
}

const bg = (page: import("@playwright/test").Page) => page.evaluate(() => getComputedStyle(document.body).backgroundColor);

test("the look follows the system on auto, and a choice is kept with the account", async ({ page, request, adminToken }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", /./);
  expect(await bg(page)).toBe("rgb(14, 17, 22)");
  await page.emulateMedia({ colorScheme: "light" });
  expect(await bg(page)).toBe("rgb(245, 246, 248)");

  // Dark, chosen: kept over the system's light, and over a reload.
  const theme = page.getByRole("group", { name: "theme" });
  await theme.getByRole("button", { name: "Dark" }).click();
  await expect(theme.getByRole("button", { name: "Dark" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  expect(await bg(page)).toBe("rgb(14, 17, 22)");
  await savedTheme(request, adminToken, "dark");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  // The text and the canvas follow: CodeMirror's own dark flag, React Flow's colour mode.
  await openSample(page, "Hello API", false);
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible();
  await expect(page.locator(".hl-canvas__flow > .react-flow")).toHaveClass(/dark/);
  await theme.getByRole("button", { name: "Light" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator(".hl-canvas__flow > .react-flow")).toHaveClass(/light/);

  // Back to auto, for the tests after this one.
  await theme.getByRole("button", { name: "Auto" }).click();
  await savedTheme(request, adminToken, "auto");
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", /./);
});
