// The console at phone width and in the Terminal interface (gaps-web.md G33, G34; styles.css's
// breakpoints, look.ts's data-ui): at 390 px the overview, a version's review, the editor and a
// flow's history and Status each fit across, with no sideways scroll, the nav is a drawer the top bar's menu
// opens, and the editor shows one pane at a time. `t` switches to Terminal (mono, square, a status
// bar) and back, kept with the account. HOLON_E2E_SHOTS=<dir> also takes screenshots of each.

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page } from "@playwright/test";
import { expect, SERVER, test } from "./fixtures";
import { openSample } from "./helpers";

const root = join(fileURLToPath(import.meta.url), "..", "..", "..");
const HOLON = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
const holon = (token: string, args: string[]) => execFileSync(HOLON, [...args, "--server", SERVER], { env: { ...process.env, HOLON_TOKEN: token }, encoding: "utf8", stdio: "pipe" });
const SHOTS = process.env.HOLON_E2E_SHOTS;
const shot = async (page: Page, name: string) => {
  if (SHOTS) await page.screenshot({ path: join(SHOTS, `${name}.png`) });
};

/** Nothing scrolls sideways: not the page, nor the room a page has over the canvas. */
async function fitsAcross(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(() =>
        [document.documentElement, ...document.querySelectorAll(".hl-pagebox")]
          .filter((el) => el.clientWidth > 0)
          .map((el) => (el.scrollWidth <= el.clientWidth + 1 ? "fits" : `${el.className || el.tagName} is ${el.scrollWidth} wide in ${el.clientWidth}`))
          .filter((s) => s !== "fits")
          .concat(document.documentElement.scrollWidth <= window.innerWidth ? [] : [`the page is ${document.documentElement.scrollWidth} wide`]),
      ),
    )
    .toEqual([]);
}

/** Two flows of its own: one deployed (so it has a history), one saved and waiting for review. */
function twoFlows(token: string) {
  const id = Date.now().toString(36);
  const dir = mkdtempSync(join(tmpdir(), "holon-e2e-narrow-"));
  const text = (name: string) => `import { action, flow } from "holon"\n\nflow("${name}")\n\nexport const shout = action((input) => input.toUpperCase())\n`;
  const live = `narrow-${id}`;
  const waits = `narrow-review-${id}`;
  writeFileSync(join(dir, "live.holon.ts"), text(live));
  writeFileSync(join(dir, "waits.holon.ts"), text(waits));
  holon(token, ["deploy", join(dir, "live.holon.ts"), "--approve"]);
  holon(token, ["action", live, "shout", "--input", '"hi"']);
  holon(token, ["flows", "save", join(dir, "waits.holon.ts")]);
  return { live, waits };
}

async function savedUi(request: APIRequestContext, token: string, ui: string) {
  const headers = { Authorization: `Bearer ${token}` };
  await expect.poll(async () => (await (await request.get(`${SERVER}/auth/prefs`, { headers })).text()).includes(`"ui":"${ui}"`)).toBe(true);
}

test.describe("at 390 px", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("the overview, a review, the editor and a flow's history fit across; the nav is a drawer", async ({ page, adminToken }) => {
    const { live, waits } = twoFlows(adminToken);
    try {
      // The overview, and the nav out of the way until the menu opens it.
      await page.goto("/#overview");
      await expect(page.getByRole("heading", { level: 1, name: /^Good (morning|afternoon|evening), e2e$/ })).toBeVisible();
      const nav = page.getByRole("navigation", { name: "main" });
      await expect(nav.getByRole("link", { name: "Flows", exact: true })).toBeHidden();
      await fitsAcross(page);
      await shot(page, "phone-overview");
      const menu = page.getByRole("button", { name: "menu" });
      await menu.click();
      await expect(menu).toHaveAttribute("aria-expanded", "true");
      await expect(nav.getByRole("link", { name: "Flows", exact: true })).toBeVisible();
      await shot(page, "phone-drawer");
      await nav.getByRole("link", { name: /^Approvals/ }).click();
      await expect(page).toHaveURL(/#approvals$/);
      await expect(nav.getByRole("link", { name: "Flows", exact: true })).toBeHidden();
      await fitsAcross(page);

      // A version's review: its columns one under another.
      await page.goto(`/#approvals.${waits}.v1`);
      await expect(page.getByRole("region", { name: "what it may do" })).toBeVisible();
      await expect(page.getByRole("complementary", { name: "decision" })).toBeVisible();
      await fitsAcross(page);
      await shot(page, "phone-review");

      // The editor: one pane at a time, the switcher in its bar picking which.
      await openSample(page, "Hello API");
      const canvas = page.locator(".hl-canvas");
      const panes = canvas.getByRole("group", { name: "pane shown" });
      await expect(panes.getByRole("button", { name: "canvas" })).toHaveAttribute("aria-pressed", "true");
      await expect(canvas.locator(".hl-canvas__flow")).toBeVisible();
      await expect(page.getByRole("complementary", { name: "Inspector" })).toBeHidden();
      await fitsAcross(page);
      await shot(page, "phone-editor");
      await panes.getByRole("button", { name: "text" }).click();
      await expect(page.getByRole("region", { name: "Text" }).locator(".cm-content")).toBeVisible();
      await expect(canvas.locator(".hl-canvas__flow")).toBeHidden();
      await fitsAcross(page);
      await panes.getByRole("button", { name: "inspector" }).click();
      await expect(page.getByRole("complementary", { name: "Inspector" })).toBeVisible();
      await expect(page.getByRole("region", { name: "Text" })).toBeHidden();
      await fitsAcross(page);
      await panes.getByRole("button", { name: "bottom" }).click();
      await expect(page.getByRole("region", { name: "bottom" }).getByRole("tab", { name: "Try a request" })).toBeVisible();
      await expect(page.getByRole("complementary", { name: "Inspector" })).toBeHidden();
      await fitsAcross(page);

      // A flow's history: its timeline, a table that scrolls inside itself if it must.
      await page.goto(`/#${live}.history`);
      await expect(page.getByLabel(`flow ${live}`)).toBeVisible();
      await expect(page.getByRole("region", { name: "history" })).toContainText("deployed version 1");
      await fitsAcross(page);
      await shot(page, "phone-history");

      // Its Status: the runs hour by hour as wide as there is room for.
      await page.goto(`/#${live}`);
      const day = page.getByRole("region", { name: "last 24 hours" });
      await expect(day.getByRole("img")).toBeVisible();
      expect(await day.evaluate((s) => s.querySelector(".hl-spark")!.getBoundingClientRect().right - s.getBoundingClientRect().right)).toBeLessThanOrEqual(0);
      await fitsAcross(page);
    } finally {
      const headers = { Authorization: `Bearer ${adminToken}` };
      await page.request.delete(`${SERVER}/deployments/${live}`, { headers });
      for (const name of [waits, live]) await page.request.delete(`${SERVER}/flows/${name}`, { headers });
    }
  });
});

test("t switches to the Terminal interface, with its status bar, and back; kept with the account", async ({ page, request, adminToken }) => {
  await page.goto("/#overview");
  await expect(page.getByRole("heading", { level: 1, name: /^Good / })).toBeVisible();
  await expect(page.getByRole("contentinfo", { name: "status" })).toHaveCount(0);
  await page.keyboard.press("t");
  await expect(page.locator("html")).toHaveAttribute("data-ui", "b");
  await expect(page.getByRole("region", { name: "notices" })).toContainText("Terminal interface");
  const status = page.getByRole("contentinfo", { name: "status" });
  await expect(status).toContainText(/\d+ flows? · \d+ live/);
  await expect(status).toContainText("waits:");
  await expect(status).toContainText(/chain (\S+ )?(intact|\d+ breaks?)|chain empty/);
  await expect(page.getByRole("group", { name: "interface" }).getByRole("button", { name: "Terminal" })).toHaveAttribute("aria-pressed", "true");
  // Mono and square, the brackets not read out: a page's title and a button keep their names.
  expect(await page.evaluate(() => getComputedStyle(document.body).fontFamily)).toMatch(/^"JetBrains Mono"/);
  expect(await page.evaluate(() => getComputedStyle(document.querySelector(".hl-top__find")!).borderTopLeftRadius)).toBe("0px");
  await expect(page.getByRole("heading", { level: 1, name: /^Good (morning|afternoon|evening), e2e$/ })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "main" }).getByRole("button", { name: "new flow" })).toBeVisible();
  await savedUi(request, adminToken, "terminal");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-ui", "b");
  await shot(page, "terminal-overview");

  // The editor in Terminal: its pane keys on show, the status bar's keys the editor's.
  await openSample(page, "Hello API", false);
  await expect(page.getByRole("contentinfo", { name: "status" })).toContainText("deploy");
  await expect(page.locator(".hl-canvas__flow > .react-flow")).toHaveClass(/dark/);
  await shot(page, "terminal-editor");
  // Under 1180 px the nav is a rail: each page by its key, its link still named.
  await page.setViewportSize({ width: 1000, height: 720 });
  await page.goto("/#overview");
  const nav = page.getByRole("navigation", { name: "main" });
  await expect(nav.getByRole("link", { name: "Flows", exact: true })).toBeVisible();
  expect(await nav.locator(".hl-nav__link").first().evaluate((a) => a.getBoundingClientRect().width)).toBeLessThan(60);
  await fitsAcross(page);
  await shot(page, "terminal-rail");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/#overview");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await fitsAcross(page);
  await shot(page, "terminal-phone");
  await page.setViewportSize({ width: 1280, height: 720 });

  // Back to Clean from the top bar, for the tests after this one.
  await page.getByRole("group", { name: "interface" }).getByRole("button", { name: "Clean" }).click();
  await expect(page.locator("html")).not.toHaveAttribute("data-ui", /./);
  await expect(page.getByRole("contentinfo", { name: "status" })).toHaveCount(0);
  await savedUi(request, adminToken, "clean");
  await page.setViewportSize({ width: 1000, height: 720 });
  await expect(nav.getByRole("link", { name: /^Approvals/ })).toBeVisible();
  await shot(page, "clean-rail");
});
